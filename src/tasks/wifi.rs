//! Tareas de conectividad Wi-Fi, pila de red y empaquetado de telemetría.

use defmt::{info, warn};
use embassy_executor::task;
use embassy_futures::select::{select, Either};
use embassy_net::Runner;
use embassy_sync::pubsub::{DynPublisher, DynSubscriber};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::scan::ScanConfig;
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{Config, Interface, WifiController};

use crate::events::telemetry::TelemetryMessage;
use crate::events::{Measurements, SensorState};

#[task]
pub async fn net_task(mut runner: Runner<'static, Interface<'static>>) {
    info!("Iniciando pila de red Embassy...");
    runner.run().await
}

#[task]
pub async fn wifi_connection_task(
    mut wifi: WifiController<'static>,
    ssid: &'static str,
    password: &'static str,
) -> ! {
    let sta_config = Config::Station(
        StationConfig::default()
            .with_ssid(ssid)
            .with_password(password.into()),
    );
    let scan_config = ScanConfig::default().with_max(10);
    wifi.set_config(&sta_config).unwrap();
    Timer::after(Duration::from_secs(2)).await;

    loop {
        info!("[WIFI] Conectando a red Wi-Fi: {}", ssid);
        if let Ok(networks) = wifi.scan_async(&scan_config).await {
            for ap in networks {
                info!(
                    "[WIFI SCAN] Red detectada: {} (RSSI: {} dBm)",
                    ap.ssid.as_str(),
                    ap.signal_strength
                );
            }
        }

        wifi.set_config(&sta_config).unwrap();
        if wifi.connect_async().await.is_err() {
            warn!("[WIFI] No se pudo conectar a la red Wi-Fi '{}'", ssid);
        }

        if wifi.is_connected() {
            info!("[WIFI] Conexión Wi-Fi establecida y en línea");
            match wifi.wait_for_disconnect_async().await {
                Ok(info) => {
                    warn!("[WIFI] Wi-Fi desconectado: {}", info.reason);
                }
                Err(e) => {
                    warn!("[WIFI] Error durante la desconexión: {}", e);
                }
            }
        } else {
            warn!("[WIFI] Wi-Fi fuera de línea. Reintentando...");
            Timer::after(Duration::from_secs(5)).await;
        }
    }
}

#[task]
pub async fn telemetry_task(
    mut sensors_channel: DynSubscriber<'static, Measurements>,
    telemetry_channel: DynPublisher<'static, TelemetryMessage>,
    interval_seconds: u64,
) {
    info!(
        "Tarea de telemetría iniciada con intervalo de {} s",
        interval_seconds
    );

    let mut sensor_state = SensorState::default();
    let mut snapshot_timer = Timer::after(Duration::from_secs(interval_seconds));

    loop {
        match select(sensors_channel.next_message_pure(), &mut snapshot_timer).await {
            Either::First(m) => {
                match m {
                    Measurements::BME280((t, h, p)) => sensor_state.update_bme280(t, h, p),
                    Measurements::BH1750(l) => sensor_state.update_lux(l),
                    Measurements::PH(p) => sensor_state.update_ph(p),
                    Measurements::TDS(t) => sensor_state.update_tds(t),
                    Measurements::SoilMoisture(sm) => sensor_state.update_soil_moisture(sm),
                }
            }

            Either::Second(_) => {
                if !sensor_state.is_empty() {
                    telemetry_channel.publish_immediate(TelemetryMessage::SendAll(sensor_state));
                }
                snapshot_timer = Timer::after(Duration::from_secs(interval_seconds));
            }
        }
    }
}
