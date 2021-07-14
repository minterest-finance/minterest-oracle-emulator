use codec::Decode;
use sp_keyring::AccountKeyring;

use codec::Compact;
use minterest_primitives::{currency::TokenSymbol, Balance, CurrencyId, Price};
use node_minterest_runtime::Event;

use frame_system::EventRecord;
use sp_core::crypto::{Pair, Public};
use sp_core::H256 as Hash;
use std::convert::TryFrom;
use std::fmt::Debug;
use std::{thread, time};
use substrate_api_client::{
    compose_call, compose_extrinsic, utils::FromHexString, Api, Metadata, UncheckedExtrinsicV4,
    XtStatus,
};

use std::sync::mpsc::channel;
pub const ETH: CurrencyId = CurrencyId::UnderlyingAsset(TokenSymbol::ETH);

// TODO get last feed round

// This function for testing purpose to automate add oracles
fn create_feeds() {
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
        ETH,
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

fn submit_new_value(round_id: u32, value: u32) {
    let url = "127.0.0.1:9944";
    let signer = AccountKeyring::Charlie.pair();
    let api = Api::new(format!("ws://{}", url))
        .map(|api| api.set_signer(signer.clone()))
        .unwrap();

    let xt: UncheckedExtrinsicV4<_> = compose_extrinsic!(
        api.clone(),
        "ChainlinkFeed",
        "submit",
        Compact(0_u32),
        Compact(round_id),
        Compact(value)
    );

    let tx_hash = api
        .send_extrinsic(xt.hex_encode(), XtStatus::InBlock)
        .unwrap();
    println!("[+] Transaction got included. Hash: {:?}", tx_hash);
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
                                    println!("INITIATE NEW ROUND CAUGHT!");
                                }
                                _ => {
                                    println!("ignoring unsupported balances event");
                                }
                            }
                        }
                        _ => println!("ignoring unsupported module event: {:?}", evr.event),
                    }
                }
            }
            Err(_) => println!("couldn't decode event record list"),
        }
    }
}

fn main() {
    create_feeds();

    let handler = thread::spawn(|| minterest_event_listener());

    for n in 1..101 {
        let ten_sec = time::Duration::from_millis(10000);
        thread::sleep(ten_sec);
        submit_new_value(n, n);
    }

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
