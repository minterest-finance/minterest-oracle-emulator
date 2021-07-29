use codec::Decode;
use sp_keyring::AccountKeyring;

use codec::Compact;
use minterest_primitives::{currency::*, CurrencyId};
use node_minterest_runtime::{Balance, BlockNumber, Event, Header};

use coingecko::{Client, SimplePrice, SimplePriceReq, SimplePrices};
use frame_system::EventRecord;
use futures::executor::block_on;
use rust_decimal::prelude::*;
use sp_core::crypto::{Pair, Public};
use sp_core::H256 as Hash;
use sp_runtime::FixedU128;
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

fn get_feed_id(currency_id: CurrencyId) -> u32 {
    match currency_id {
        ETH => 0,
        DOT => 1,
        KSM => 2,
        BTC => 3,
        _ => panic!(),
    }
}

fn get_feed_descrtiption(currency_id: CurrencyId) -> &'static str {
    match currency_id {
        ETH => "MIN-ETH",
        DOT => "MIN-DOT",
        KSM => "MIN-KSM",
        BTC => "MIN-BTC",
        _ => panic!(),
    }
}

// This function for testing purpose to automate add oracles
fn create_feeds() {
    println!("Start feed creating. Don't interupt the service!");
    create_chainlink_feed(ETH);
    create_chainlink_feed(DOT);
    create_chainlink_feed(KSM);
    create_chainlink_feed(BTC);
    println!("Finish feed creating");
    // sequence is important! (see get_feed_id)
}

fn create_chainlink_feed(currency_id: CurrencyId) {
    let url = "127.0.0.1:9944";
    let signer = AccountKeyring::Alice.pair();
    let api = Api::new(format!("ws://{}", url))
        .map(|api| api.set_signer(signer.clone()))
        .unwrap();

    let oracle_admin = AccountKeyring::Bob.to_account_id();
    let oracle = AccountKeyring::Charlie.to_account_id();

    let xt: UncheckedExtrinsicV4<_> = compose_extrinsic!(
        api.clone(),
        "ChainlinkFeed",
        "create_feed",
        0_u128,                             // payment
        0 as BlockNumber,                   // timeout
        (0_u128, u128::MAX),                // submission_value_bounds
        1_u32,                              // min submission
        0_u8,                               // decimals
        get_feed_descrtiption(currency_id), // description
        0_u32,                              // restart delay
        vec![(oracle, oracle_admin)],       // oracles
        Option::<u32>::None,                // prunning window
        Option::<u128>::None                // max debt
    );

    let tx_hash = api
        .send_extrinsic(xt.hex_encode(), XtStatus::InBlock)
        .unwrap();
    println!("[+] Transaction got included. Hash: {:?}", tx_hash);
}

static mut nonce: u32 = 0;

fn submit_new_value(feed_id: u32, round_id: u32, value: u128) {
    let url = "127.0.0.1:9944";
    let signer = AccountKeyring::Charlie.pair();
    let api = Api::new(format!("ws://{}", url))
        .map(|api| api.set_signer(signer.clone()))
        .unwrap();

    unsafe {
        if api.get_nonce().unwrap() > nonce {
            // it covers the case when service was restarted, and nonce should be greater than zero
            nonce = api.get_nonce().unwrap();
        }
    }

    let call = compose_call!(
        api.metadata.clone(),
        "ChainlinkFeed",
        "submit",
        Compact(feed_id),
        Compact(round_id),
        Compact(value)
    );

    // Information for Era for mortal transactions
    let head = api.get_finalized_head().unwrap().unwrap();
    let h: Header = api.get_header(Some(head)).unwrap().unwrap();
    let period = 5;

    unsafe {
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
        let tx_hash = api
            .send_extrinsic(xt.hex_encode(), XtStatus::Ready)
            .unwrap();
        println!("[+] Transaction got included. Hash: {:?}", tx_hash);
    }
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

fn underlying_to_string(cur_id: CurrencyId) -> &'static str {
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
                    // println!("decoded: {:?} {:?}", evr.phase, evr.event);
                    match &evr.event {
                        Event::ChainlinkPriceManager(be) => {
                            println!(">>>>>>>>>> chainlink price manager event: {:?}", be);
                            match &be {
                                chainlink_price_manager::Event::InitiateNewRound(
                                    feed_id,
                                    round_id,
                                ) => {
                                    let prices = get_prices();
                                    for token in
                                        CurrencyId::get_enabled_tokens_in_protocol(UnderlyingAsset)
                                    {
                                        let token_name = underlying_to_string(token);
                                        let price = prices[token_name]["usd"];
                                        let converted_price =
                                            convert_rust_decimal_to_u128_18(&price);
                                        println!(
                                            "Token name: {:?}, price: {:?}, converted_price: {:?}",
                                            underlying_to_string(token),
                                            price,
                                            converted_price
                                        );

                                        submit_new_value(
                                            get_feed_id(token),
                                            *round_id,
                                            converted_price,
                                        );
                                    }
                                }
                                _ => {
                                    println!("ignoring unsupported balances event");
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => println!("couldn't decode event record list"),
        }
    }
}

// If at least one feed was created we assume that this service already created feeds
fn is_feeds_were_created() -> bool {
    let url = "127.0.0.1:9944";
    let signer = AccountKeyring::Charlie.pair();
    let api = Api::new(format!("ws://{}", url))
        .map(|api| api.set_signer(signer.clone()))
        .unwrap();

    let feeds: Option<u32> = api
        .get_storage_value("ChainlinkFeed", "FeedCounter", None)
        .unwrap();

    // if any feeds wan't created, rpc aboce returns Option<None>
    !feeds.is_none()
}

fn convert_rust_decimal_to_u128_18(val: &Decimal) -> u128 {
    let multiplier = Decimal::new(10_i64.pow(18), 0);
    let integer = val.trunc() * multiplier;
    let fract = (val.fract() * multiplier).trunc();
    // TODO check value overflow
    (integer + fract).to_u128().unwrap()
}

fn main() {
    if !is_feeds_were_created() {
        create_feeds();
    }
    minterest_event_listener();
}
