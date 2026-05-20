use crate::events::SensorState;
use defmt::{info, warn, debug};
use embassy_futures::select::{select, Either};
use esp_radio::wifi::{Config, Interface};
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::scan::ScanConfig;
use esp_radio::wifi::WifiController;
use embassy_net::{Runner, /*Stack*/};
use embassy_time::{Duration, Timer};
use embassy_sync::pubsub::{DynPublisher, DynSubscriber};
use crate::events::{Measurements};
use crate::events::telemetry::TelemetryMessage;

#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    info!("Starting network stack");
    runner.run().await
}

#[embassy_executor::task]
pub async fn wifi_connection_task(mut wifi: WifiController<'static>, ssid: &'static str, password: &'static str) -> ! {
    let sta_config = Config::Station(StationConfig::default()
        .with_ssid(ssid)
        .with_password(password.into())
    );
    let scan_config = ScanConfig::default().with_max(10);
    wifi.set_config(&sta_config).unwrap();
    Timer::after(Duration::from_secs(2)).await;

    loop {
        info!("[WIFI] Connecting to wifi...");
        info!("[WIFI] Scanning for Wifi Networks");
        // todo: channel send state
        let networks = wifi.scan_async(&scan_config).await.unwrap();
        for ap in networks {
            info!("wifi network: {} @ {}db", ap.ssid.as_str(), ap.signal_strength);
            /*
            if let Some(cfg) = config_for_network(&n.ssid) {
                wifi.set_config(&esp_radio::wifi::ModeConfig::Client(cfg)).unwrap();
                if wifi.connect_async().await.is_err() {
                    error!("Unable to connect to wifi {n:?}");
                }
                break;
            }
            */
        }

        // TODO. actualizar para soportar diferentes redes (como esta comentado arriba)
        wifi.set_config(&sta_config).unwrap();
        if wifi.connect_async().await.is_err() {
            warn!("Unable to connect to wifi {}", ssid);
        }

        if wifi.is_connected() {
            info!("[WIFI] Wifi is online!");
            // todo: channel send wifi::connected | up | etc
            match wifi.wait_for_disconnect_async().await {
                Ok(info) => {
                    warn!("[Wifi] Wifi disconnected! {}", info.reason);
                },
                Err(e) => {
                    warn!("[WIFI] Error in disconnection to wifi: {}", e);
                }
            }
        } else {
            warn!("[WIFI] Wifi offline");
            // todo: channel send wifi::Offline or use stack
            Timer::after(Duration::from_secs(5)).await;
        }
    }
}

/*
#[embassy_executor::task]
pub async fn http_api_task(stack: Stack<'static>, mut connection_channel: DynSubscriber<'static, Measurements>) {
    let seed = Rng::new().random() as u64;
    let mut rx_buf = [0; 4096];
    let mut tx_buf = [0; 4096];

    let dns = DnsSocket::new(stack);
    let tcp_state = TcpClientState::<1, 4096, 4096>::new();
    let tcp = TcpClient::new(stack, &tcp_state);
    let tls = TlsConfig::new(
        seed,
        &mut rx_buf,
        &mut tx_buf,
        reqwless::client::TlsVerify::None
    );

    let mut client = HttpClient::new_with_tls(&tcp, &dns, tls);
    loop {
        if let Some(m) = connection_channel.try_next_message_pure() {
            stack.wait_config_up().await;
            if let Some(config) = stack.config_v4() { // todo: save ip
                info!("[HTTP API] this is my ip {}, hd: {}", config.address, stack.hardware_address());
            }
            // Todo: complete match pattern
            let mut data: String<128> = String::new();
            m.to_json(&mut data);

            make_request(&mut client, data.as_bytes()).await;
        }



        Timer::after(Duration::from_secs(10)).await;
    }
}

async fn make_request(client: &mut HttpClient<'_, TcpClient<'_, 1, 4096, 4096>, DnsSocket<'_>>, body: &[u8]) {
    info!("[WIFI REQUEST] Making a request");
    let mut buffer = [0u8; 4096];
    let url: &str = "https://rickandmortyapi.com/api";
    let http_request = client.request(
        reqwless::request::Method::POST,
        url
    ).await.unwrap();

    let mut request =  http_request.body(body);
    let response = request.send(&mut buffer).await;
    match response {
        Ok(res) => {
            let r = res.body().read_to_end().await.unwrap();
            info!("[WIFI REQUEST] Response body: {:?}", core::str::from_utf8(r).unwrap());
        }
        Err(e) => { warn!("Error while making HTTP request: {}", e); }
    }


    Timer::after(Duration::from_secs(1)).await;
}
*/

#[embassy_executor::task]
pub async fn telemetry_task(mut sensors_channel: DynSubscriber<'static, Measurements>, telemetry_channel: DynPublisher<'static, TelemetryMessage>) {
    info!("Telemetry task started");

    let mut sensor_state = SensorState::default();
    let mut snapshot_timer = Timer::after(Duration::from_secs(60));

    loop {
        match select(
            sensors_channel.next_message_pure(),
            &mut snapshot_timer
        ).await {
            Either::First(m) => {
                sensor_state.update(m);
                telemetry_channel.publish(TelemetryMessage::SendOne(m)).await;
            }

            Either::Second(_) => {
                telemetry_channel.publish_immediate(TelemetryMessage::SendAll(sensor_state));
                sensor_state.clean();
                snapshot_timer = Timer::after(Duration::from_secs(60));
            }
        }
    }
}
