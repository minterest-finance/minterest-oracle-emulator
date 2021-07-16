use codec::Decode;
use sp_keyring::AccountKeyring;

use codec::Compact;
use minterest_primitives::{currency::*, Balance, CurrencyId, Price};
use node_minterest_runtime::{Event, Header};

use coingecko::{Client, SimplePrice, SimplePriceReq, SimplePrices};
use frame_system::EventRecord;
use futures::executor::block_on;
use rust_decimal::prelude::*;
use sp_core::crypto::{Pair, Public};
use sp_core::H256 as Hash;
use std::convert::TryFrom;
use std::fmt::Debug;
use std::{thread, time};
use substrate_api_client::{
    compose_call, compose_extrinsic, compose_extrinsic_offline, utils::FromHexString, Api,
    Metadata, UncheckedExtrinsicV4, XtStatus,
};

use minterest_primitives::currency::CurrencyType::UnderlyingAsset;
use std::collections::HashMap;
use std::sync::mpsc::channel;

// TODO get last feed round

// This function for testing purpose to automate add oracles
fn create_feeds(currency_id: CurrencyId) {
    let url = "127.0.0.1:9944";
    let signer = AccountKeyring::Alice.pair();
    let api = Api::new(format!("ws://{}", url))
        .map(|api| api.set_signer(signer.clone()))
        .unwrap();

    let oracle_admin = AccountKeyring::Bob.to_account_id();
    let oracle = AccountKeyring::Charlie.to_account_id();

    let call = compose_call!(
        api.metadata.clone(),
        "ChainlinkPriceManager",
        "create_minterest_feed",
        currency_id,
        1,
        vec![(oracle, oracle_admin)]
    );

    let xt: UncheckedExtrinsicV4<_> = compose_extrinsic!(api.clone(), "Sudo", "sudo", call);

    // send and watch extrinsic until finalized
    let tx_hash = api
        .send_extrinsic(xt.hex_encode(), XtStatus::InBlock)
        .unwrap();
    println!("[+] Transaction got included. Hash: {:?}", tx_hash);
}

static mut nonce: u32 = 0;

fn submit_new_value(round_id: u32, value: u32, cur_id: CurrencyId) {
    let url = "127.0.0.1:9944";
    let signer = AccountKeyring::Charlie.pair();
    let api = Api::new(format!("ws://{}", url))
        .map(|api| api.set_signer(signer.clone()))
        .unwrap();

    let call = compose_call!(
        api.metadata.clone(),
        "ChainlinkPriceManager",
        "submit",
        cur_id,
        Compact(round_id),
        Compact(value)
    );

    // Information for Era for mortal transactions
    let head = api.get_finalized_head().unwrap().unwrap();
    let h: Header = api.get_header(Some(head)).unwrap().unwrap();
    let period = 5;

    unsafe {
        #[allow(clippy::redundant_clone)]
        let xt: UncheckedExtrinsicV4<_> = compose_extrinsic_offline!(
            api.clone().signer.unwrap(),
            call.clone(),
            nonce,
            Era::mortal(period, h.number.into()),
            api.genesis_hash,
            head,
            api.runtime_version.spec_version,
            api.runtime_version.transaction_version
        );
        nonce += 1;
        api.send_extrinsic(xt.hex_encode(), XtStatus::Ready)
            .unwrap();
    }
    // let xt: UncheckedExtrinsicV4<_> = compose_extrinsic!(
    //     api.clone(),
    //     "ChainlinkPriceManager",
    //     "submit",
    //     cur_id,
    //     Compact(round_id),
    //     Compact(value)
    // );

    // println!("[+] Transaction got included. Hash: {:?}", tx_hash);
}

fn get_prices() -> SimplePrices {
    block_on(async {
        let http = isahc::HttpClient::new().unwrap();

        let client = Client::new(http);

        let req = SimplePriceReq::new("ethereum,polkadot,bitcoin,kusama".into(), "usd".into())
            .include_market_cap()
            .include_24hr_vol()
            .include_24hr_change()
            .include_last_updated_at();
        client.simple_price(req).await
    })
    .unwrap()
}

fn uderlying_to_string(cur_id: CurrencyId) -> &'static str {
    // ethereum,polkadot,bitcoin,kusama"
    match cur_id {
        ETH => "ethereum",
        DOT => "polkadot",
        KSM => "kusama",
        BTC => "bitcoin",
        _ => panic!(),
    }
}

fn minterest_event_listener() {
    let url = "127.0.0.1:9944";
    let signer = AccountKeyring::Charlie.pair();
    let api = Api::new(format!("ws://{}", url))
        .map(|api| api.set_signer(signer.clone()))
        .unwrap();

    println!("Subscribe to events");
    let (events_in, events_out) = channel();
    api.subscribe_events(events_in).unwrap();

    loop {
        let event_str = events_out.recv().unwrap();

        let _unhex = Vec::from_hex(event_str).unwrap();
        let mut _er_enc = _unhex.as_slice();
        let _events = Vec::<EventRecord<Event, Hash>>::decode(&mut _er_enc);
        // TODO should we need to wait only for events from finalized block?
        match _events {
            Ok(evts) => {
                for evr in &evts {
                    println!("decoded: {:?} {:?}", evr.phase, evr.event);
                    match &evr.event {
                        Event::ChainlinkPriceManager(be) => {
                            println!(">>>>>>>>>> chainlink price manager event: {:?}", be);
                            match &be {
                                chainlink_price_manager::Event::InitiateNewRound(
                                    feed_id,
                                    round_id,
                                ) => {
                                    let prices = get_prices();
                                    println!("Prices: {:?}", prices);

                                    for token in
                                        CurrencyId::get_enabled_tokens_in_protocol(UnderlyingAsset)
                                    {
                                        let token_name = uderlying_to_string(token);
                                        let val = prices[token_name]["usd"].to_u32();
                                        let float_val = prices[token_name]["usd"].to_f64();
                                        println!(
                                            "Token name: {:?}, price: {:?}",
                                            token_name, float_val
                                        );

                                        submit_new_value(*round_id, val.unwrap(), token);
                                    }
                                    // submit_new_value(*round_id, *round_id);
                                }
                                _ => {
                                    println!("ignoring unsupported balances event");
                                }
                            }
                        }
                        _ => {} // println!("ignoring unsupported module event: {:?}", evr.event),
                    }
                }
            }
            Err(_) => println!("couldn't decode event record list"),
        }
    }
}

fn main() {
    // let minterest_node_addr = std::env::args().nth(1).expect(
    //     "Expect minterest node address as first argument. \
    //     example usage: ./minterest-oracle 127.0.0.1:9944",
    // );

    // println!("Minterest node address: {:?}", minterest_node_addr);

    create_feeds(ETH);
    create_feeds(BTC);
    create_feeds(DOT);
    create_feeds(KSM);
    minterest_event_listener();

    // for token in CurrencyId::get_enabled_tokens_in_protocol(UnderlyingAsset) {
    //     println!("Token: {:?}", token);
    // }
    // let res = block_on(async {
    //     let http = isahc::HttpClient::new().unwrap();

    //     let client = Client::new(http);

    //     let req = SimplePriceReq::new("ethereum,polkadot,bitcoin,kusama".into(), "usd".into())
    //         .include_market_cap()
    //         .include_24hr_vol()
    //         .include_24hr_change()
    //         .include_last_updated_at();
    //     client.simple_price(req).await
    // })
    // .unwrap();

    let res = get_prices();
    println!("{:#?}", res["ethereum"]["usd"]);
    println!("{:#?}", res["ethereum"]["usd"].to_u128());
    // let handler = thread::spawn(|| minterest_event_listener());

    // for n in 1..101 {
    //     let ten_sec = time::Duration::from_millis(10000);
    //     thread::sleep(ten_sec);
    //     submit_new_value(n, n);
    // }

    // let url = "127.0.0.1:9944";
    // let signer = AccountKeyring::Charlie.pair();
    // let api = Api::new(format!("ws://{}", url))
    //     .map(|api| api.set_signer(signer.clone()))
    //     .unwrap();

    // let (sender, receiver) = channel();
    // api.subscribe_finalized_heads(sender).unwrap();

    // for _ in 0..5 {
    //     let head: Header = receiver
    //         .recv()
    //         .map(|header| serde_json::from_str(&header).unwrap())
    //         .unwrap();
    //     println!("Got new Block {:?}", head);
    // }
}
