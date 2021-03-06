use codec::{Compact, Decode};
use coingecko::{Client, SimplePriceReq, SimplePrices};
use frame_system::EventRecord;
use futures::executor::block_on;
use log::{info, LevelFilter};
use minterest_primitives::{currency::*, CurrencyId};
use minterest_standalone_runtime::{BlockNumber, Event, Header};
use rust_decimal::prelude::*;
use simple_logger::SimpleLogger;
use sp_core::crypto::Pair;
use sp_core::H256 as Hash;
use sp_keyring::AccountKeyring;
use std::{
    collections::HashMap,
    env,
    sync::mpsc::{channel, Receiver},
    thread, time,
};
use substrate_api_client::{
    compose_call, compose_extrinsic, compose_extrinsic_offline, utils::FromHexString, Api,
    UncheckedExtrinsicV4, XtStatus,
};

// request for coinmarketcap (sandbox key)
// curl -H "X-CMC_PRO_API_KEY: b54bcf4d-1bca-4e8e-9a24-22ff2c3d462c" -H "Accept: application/json" -d "symbol=ETH,BTC,DOT,KSM" -G https://sandbox-api.coinmarketcap.com/v1/cryptocurrency/quotes/latest
// get your own API key and use COINMARKETCAP_API_URL to use real data
// const COINMARKETCAP_API_URL: &'static str = "https://pro-api.coinmarketcap.com/v1/cryptocurrency/quotes/latest"

// coinmarketcap api key (demo env)
const COINMARKETCAP_DEMO_KEY: &'static str = "b54bcf4d-1bca-4e8e-9a24-22ff2c3d462c";
const COINMARKETCAP_DEMO_API_URL: &'static str =
    "https://sandbox-api.coinmarketcap.com/v1/cryptocurrency/quotes/latest";

#[derive(Default, Debug, Clone, PartialEq, serde_derive::Serialize, serde_derive::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub data: HashMap<String, Data>,
}

#[derive(Default, Debug, Clone, PartialEq, serde_derive::Serialize, serde_derive::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Data {
    pub quote: Quote,
}

#[derive(Default, Debug, Clone, PartialEq, serde_derive::Serialize, serde_derive::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    #[serde(rename = "USD")]
    pub usd: Usd,
}

#[derive(Default, Debug, Clone, PartialEq, serde_derive::Serialize, serde_derive::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usd {
    pub price: Decimal,
}

fn get_currency_id(feed_id: u32) -> CurrencyId {
    // See create_feeds
    match feed_id {
        0 => ETH,
        1 => DOT,
        2 => KSM,
        3 => BTC,
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

fn get_prices_coinmarketcap() -> HashMap<String, Decimal> {
    let response = reqwest::blocking::Client::new()
        .get(COINMARKETCAP_DEMO_API_URL)
        .header("X-CMC_PRO_API_KEY", COINMARKETCAP_DEMO_KEY)
        .header("Accept", "application/json")
        .query(&[("symbol", "ETH,DOT,KSM,BTC")])
        .send()
        .unwrap()
        .text()
        .unwrap();

    let deserialized: Root = serde_json::from_str(&response).unwrap();
    deserialized
        .data
        .into_iter()
        .map(|(key, value)| (key, value.quote.usd.price))
        .collect()
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

fn convert_rust_decimal_to_u128_18(val: &Decimal) -> u128 {
    let multiplier = Decimal::new(10_i64.pow(18), 0);
    let integer = val.trunc() * multiplier;
    let fract = (val.fract() * multiplier).trunc();
    // TODO check value overflow
    (integer + fract).to_u128().unwrap()
}

#[derive(Clone)]
struct Configuration {
    pub node_url: String,
}

// Main struct of minterest-oracle service
struct Service {
    pub configuration: Configuration,
    api_nonce: u32,
}

impl Configuration {
    pub fn new() -> Self {
        let args: Vec<String> = env::args().collect();
        if args.len() != 2 {
            panic!("Required minterest-node address argument. Usage example: ./minterest-oracle 127.0.0.1:9944")
        }
        Configuration {
            node_url: format!("ws://{}", args[1]),
        }
    }
}

impl Service {
    // This function for testing purpose to automate add oracles
    fn create_feeds(&self) {
        // sequence is important! (see get_feed_id and get_currency_id)
        log::info!("Start feed creating. Don't interrupt the service!");
        self.create_chainlink_feed(ETH);
        self.create_chainlink_feed(DOT);
        self.create_chainlink_feed(KSM);
        self.create_chainlink_feed(BTC);
        log::info!("Feed creating is finished");
    }

    // If at least one feed was created we assume that this service already created feeds
    fn is_feeds_were_created(&self) -> bool {
        let signer = AccountKeyring::Charlie.pair();
        let api = Api::new(self.configuration.node_url.to_string())
            .map(|api| api.set_signer(signer.clone()))
            .unwrap();

        let feeds: Option<u32> = api
            .get_storage_value("ChainlinkFeed", "FeedCounter", None)
            .unwrap();

        // if any feeds wan't created, rpc above returns Option<None>
        !feeds.is_none()
    }

    fn create_chainlink_feed(&self, currency_id: CurrencyId) {
        let signer = AccountKeyring::Alice.pair();
        let api = Api::new(self.configuration.node_url.to_string())
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

    pub fn init() -> Self {
        SimpleLogger::new()
            .with_level(LevelFilter::Info)
            .with_module_level("substrate_api_client", LevelFilter::Warn)
            .with_module_level("ws", LevelFilter::Warn)
            .init()
            .unwrap();

        let conf = Configuration::new();
        Service {
            configuration: conf.clone(),
            api_nonce: Self::get_api_nonce(&conf.clone().node_url),
        }
    }

    fn start_listen_events<T>(&self) -> Option<Receiver<String>> {
        let signer = AccountKeyring::Charlie.pair();
        let api = Api::new(self.configuration.node_url.clone())
            .map(|api| api.set_signer(signer.clone()))
            .ok()?;

        let (events_in, events_out) = channel();
        api.subscribe_events(events_in).ok()?;
        log::info!("Subscribed to events");
        Some(events_out)
    }

    fn listen_minterest_events(&mut self) {
        let mut events_out = self.start_listen_events::<String>().unwrap();
        loop {
            let event_str = events_out.recv();
            if event_str.is_err() {
                log::error!(
                    "Recieve event error. Minterest node shutd down or connection is lost.
                 Trying to reconnect"
                );
                let eo = self.start_listen_events::<String>();
                if !eo.is_none() {
                    events_out = eo.unwrap();
                }
                // Timeout 1 second for reconnection
                thread::sleep(time::Duration::from_millis(1000));
                continue;
            }
            let event_str = event_str.unwrap(); // it is safely unwrap because previous checking

            let _unhex = Vec::from_hex(event_str).unwrap();
            let mut er_enc = _unhex.as_slice();
            let events = Vec::<EventRecord<Event, Hash>>::decode(&mut er_enc);
            match events {
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

                                        let token_name =
                                            underlying_to_string(get_currency_id(*feed_id));
                                        let price = prices[token_name]["usd"];
                                        let converted_price =
                                            convert_rust_decimal_to_u128_18(&price);
                                        log::info!(
                                            "Token name: {:?}, Round: {:?}, price: {:?}, converted_price: {:?}",
                                            token_name,
                                            round_id,
                                            price,
                                            converted_price
                                        );

                                        // TODO: call coinmarketcap instead if needed
                                        // let coinmarketcap_prices = get_prices_coinmarketcap();
                                        // log::info!("coinmarketcap prices : {:?}", coinmarketcap_prices);

                                        self.submit_new_value(*feed_id, *round_id, converted_price);
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

    /// We keep API nonce by ourselves to have possibility to send more than 1 extrinsic without
    /// waiting for response
    fn get_api_nonce(url: &String) -> u32 {
        let signer = AccountKeyring::Charlie.pair();
        let api = Api::new(url.to_string())
            .map(|api| api.set_signer(signer.clone()))
            .unwrap();
        api.get_nonce().unwrap()
    }

    fn submit_new_value(&mut self, feed_id: u32, round_id: u32, value: u128) {
        let signer = AccountKeyring::Charlie.pair();
        let api = Api::new(self.configuration.node_url.clone())
            .map(|api| api.set_signer(signer.clone()))
            .unwrap();

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

        let xt: UncheckedExtrinsicV4<_> = compose_extrinsic_offline!(
            api.clone().signer.unwrap(),
            call.clone(),
            self.api_nonce,
            Era::mortal(period, h.number.into()),
            api.genesis_hash,
            head,
            api.runtime_version.spec_version,
            api.runtime_version.transaction_version
        );
        self.api_nonce += 1;
        if api
            .send_extrinsic(xt.hex_encode(), XtStatus::Ready)
            .is_err()
        {
            log::error!("Fail to send extrinsic");
        }
    }

    pub fn run(&mut self) {
        if !self.is_feeds_were_created() {
            self.create_feeds();
        }
        self.listen_minterest_events();
    }
}

fn main() {
    Service::init().run();
}
