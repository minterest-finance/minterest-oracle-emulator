use codec::{Compact, Decode};
use coingecko::{Client, SimplePriceReq, SimplePrices};
use frame_system::EventRecord;
use futures::executor::block_on;
use log::{debug, info, trace, warn, LevelFilter};
use minterest_primitives::{currency::*, CurrencyId};
use node_minterest_runtime::{BlockNumber, Event, Header};
use rust_decimal::prelude::*;
use simple_logger::SimpleLogger;
use sp_core::crypto::Pair;
use sp_core::H256 as Hash;
use sp_keyring::AccountKeyring;
use substrate_api_client::{
    compose_call, compose_extrinsic, compose_extrinsic_offline, utils::FromHexString, Api,
    UncheckedExtrinsicV4, XtStatus,
};

use minterest_primitives::currency::CurrencyType::UnderlyingAsset;
use std::sync::mpsc::{channel, Receiver};
use std::{thread, time};

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
    log::info!("Start feed creating. Don't interrupt the service!");
    create_chainlink_feed(ETH);
    create_chainlink_feed(DOT);
    create_chainlink_feed(KSM);
    create_chainlink_feed(BTC);
    log::info!("Feed creating is finished");
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
    log::info!("[+] Transaction got included. Hash: {:?}", tx_hash);
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

fn start_listen_events<T>() -> Option<Receiver<String>> {
    let url = "127.0.0.1:9944";
    let signer = AccountKeyring::Charlie.pair();
    let api = Api::new(format!("ws://{}", url))
        .map(|api| api.set_signer(signer.clone()))
        .ok()?;

    let (events_in, events_out) = channel();
    api.subscribe_events(events_in).ok()?;
    log::info!("Subscribed to events");
    Some(events_out)
}

fn minterest_event_listener() {
    let mut events_out = start_listen_events::<String>().unwrap();
    loop {
        let event_str = events_out.recv();
        if event_str.is_err() {
            log::error!(
                "Recieve event error. Minterest node shutd down or connection is lost.
                 Trying to reconnect"
            );
            let eo = start_listen_events::<String>();
            if !eo.is_none() {
                events_out = eo.unwrap();
            }
            // Timeout 1 second for reconnection
            thread::sleep(time::Duration::from_millis(1000));
            continue;
        }
        let event_str = event_str.unwrap();

        let _unhex = Vec::from_hex(event_str).unwrap();
        let mut _er_enc = _unhex.as_slice();
        let _events = Vec::<EventRecord<Event, Hash>>::decode(&mut _er_enc);
        match _events {
            Ok(evts) => {
                for evr in &evts {
                    match &evr.event {
                        Event::ChainlinkPriceManager(be) => {
                            log::info!("Chainlink price manager event: {:?}", be);
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
                                        log::info!(
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
                                    log::debug!("ignoring unsupported balances event");
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(_) => log::warn!("couldn't decode event record list"),
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
    SimpleLogger::new()
        .with_level(LevelFilter::Info)
        .with_module_level("substrate_api_client", LevelFilter::Warn)
        .with_module_level("ws", LevelFilter::Warn)
        .init()
        .unwrap();

    if !is_feeds_were_created() {
        create_feeds();
    }
    minterest_event_listener();
}
