#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::{debug, info};
use esp_hal::clock::CpuClock;
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::{Trng, TrngSource};
use esp_hal::timer::timg::TimerGroup;
use esp_hal::{Async, i2c::master::{Config, I2c}, };
use esp_println::{self as _, println};
use esp_radio::wifi::ControllerConfig;
use esp_rtos::embassy::Executor;
use esp_rtos::start_second_core;
use static_cell::{ConstStaticCell, StaticCell};

use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_net::{DhcpConfig, StackResources};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::pubsub::PubSubChannel;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, mutex::Mutex};
use esp_hal::analog::adc::{Adc, AdcConfig, Attenuation};
use smartpot_iot::drivers::tca9548a::Tca9548a;

#[cfg(feature = "sensors")]
use smartpot_iot::events::SENSOR_CH;

#[cfg(feature = "actuators")]
use smartpot_iot::events::{COMMAND_CH};

extern crate alloc;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

pub const SSID: &str = env!("SSID");
pub const PASSWORD: &str = env!("PASSWORD");

//#[unsafe(no_mangle)]
//pub extern "C" fn __esp_radio_printf() -> core::ffi::c_int {
//    // TODO: quitar este machetazo de la feature "print-logs-from-driver" de esp-radio
//    0
//}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\n\n================ PANIC ================");
    if let Some(location) = info.location() {
        println!(
            "Panic in file '{}' at line {}",
            location.file(),
            location.line(),
        );
    } else {
        println!("Panic but no location information available.");
    }

    println!("message: {:?}", info.message());
    println!("======================================\n\n");

    loop {}
}

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    // generator version: 1.1.0
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);
    //esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 120000);
    esp_alloc::heap_allocator!(size: 98768);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);
    info!("Embassy initialized!");

    //
    //  SECOND CORE STACK AND MAIN FUNCTION
    //
    // TODO: use feature to wrap it

    // I2C protocol pinout
    let i2c0 = peripherals.I2C0;
    let sda = peripherals.GPIO21;
    let scl = peripherals.GPIO22;

    // Analog
    let mut adc1_config = AdcConfig::default();
    let tds_pin = peripherals.GPIO34;
    let tds = adc1_config.enable_pin(tds_pin, Attenuation::_11dB);

    let ph_pin = peripherals.GPIO35;
    let ph = adc1_config.enable_pin(ph_pin, Attenuation::_11dB);
    let adc_tds = Adc::new(peripherals.ADC1, adc1_config);

    // actuators pinout
    let water_pin = peripherals.GPIO25;
    let lights_pin = peripherals.GPIO26;
    let humidifier_pin = peripherals.GPIO5;

    let _software_interrupt = sw_int.software_interrupt2;
    let cpu1_main = move |_spawner: embassy_executor::Spawner| {
        debug!("Launching i2c sensor tasks");
        static I2C_BUS: StaticCell<Mutex<NoopRawMutex, I2c<'static, Async>>> = StaticCell::new();
        let i2c = I2c::new(i2c0, Config::default())
            .unwrap()
            .with_scl(scl)
            .with_sda(sda)
            .into_async();
        let _i2c_bus = I2C_BUS.init(Mutex::new(i2c));

        #[cfg(feature = "sensors")]
        {
            // i2c scanner
            _spawner.spawn(smartpot_iot::i2c_scanner::scan_i2c(I2cDevice::new(_i2c_bus)).expect("Failed to scan I2C task"));

            let tca = Tca9548a::new(_i2c_bus, 0x70);
            _spawner.spawn(smartpot_iot::tasks::sensors::bme280_sequential_task(
                tca,
                SENSOR_CH.dyn_publisher().unwrap(),
            ).expect("Failed to launch bme280 sensors task"));

            _spawner.spawn(smartpot_iot::tasks::sensors::bh1750_task(
                I2cDevice::new(_i2c_bus),
                SENSOR_CH.dyn_publisher().unwrap()
            ).expect("Failed to launch bh1750_sensors task"));

            _spawner.spawn(smartpot_iot::tasks::sensors::tds_task(
                adc_tds,
                tds,
                ph,
                SENSOR_CH.dyn_publisher().unwrap()
            ).expect("Failed to launch tds task"));
        }

        #[cfg(feature = "actuators")]
        {
            use esp_hal::gpio::{Output, OutputConfig, Level};

            let water = Output::new(water_pin, Level::Low, OutputConfig::default());
            let lights = Output::new(lights_pin, Level::Low, OutputConfig::default());
            let humidifier = Output::new(humidifier_pin, Level::Low, OutputConfig::default());

            _spawner.spawn(smartpot_iot::tasks::command::catch_commands(
                COMMAND_CH.dyn_subscriber().unwrap(),
                water,
                lights,
                humidifier
            ).expect("Failed to create catch_commands task"));
        }

        /*
        let tca = Tca9548a::new(i2c_bus, 0x70);
        let [_, _, ch2, ch3, ch4, _, _, _] = tca.split();
        spawner.spawn(smartpot_iot::tasks::sensors::bme280_task_tca(
            ch2,
            SENSOR_CH.dyn_publisher().unwrap(),
            0x76));
        spawner.spawn(smartpot_iot::tasks::sensors::bme280_task_tca(
            ch3,
            SENSOR_CH.dyn_publisher().unwrap(),
            0x76));
        spawner.spawn(smartpot_iot::tasks::sensors::bme280_task_tca(
            ch4,
            SENSOR_CH.dyn_publisher().unwrap(),
            0x76));
        */
    };

    #[allow(static_mut_refs)]
    {
        const CORE1_STACK_SIZE: usize = 8192;
        static CORE1_STACK: StaticCell<esp_hal::system::Stack<CORE1_STACK_SIZE>> = StaticCell::new();
        let core1_stack = CORE1_STACK.init_with(|| esp_hal::system::Stack::new());

        start_second_core(
            peripherals.CPU_CTRL,
            sw_int.software_interrupt1,
            core1_stack,
            || {
                info!("Starting second cpu (CORE1)");
                static EXECUTOR_CORE1: StaticCell<Executor> = StaticCell::new();
                let exec = EXECUTOR_CORE1.init(Executor::new());

                // Versión correcta para esp-rtos 0.3
                exec.run(|spawner: embassy_executor::Spawner| {
                    cpu1_main(spawner);
                });
            },
        );
    }

    #[cfg(feature = "wifi")]
    {
        use smartpot_iot::tasks::wifi::{net_task, wifi_connection_task};

        #[cfg(any(feature = "sensors", feature = "actuators", feature = "mqtt", feature = "http-api", feature = "ble"))]
        use smartpot_iot::{
            events::telemetry::TELEMETRY_CH,
            tasks::wifi::telemetry_task
        };

        //
        // WIFI INITIALIZATION
        //
        info!("Setting up network _stack");
        //static WIFI_INIT: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
        //let radio_init = WIFI_INIT.init_with(|| { esp_radio::init().expect("Failed to initialize radio controller") });

        //let (wifi_controller, interfaces) = esp_radio::wifi::new(radio_init, peripherals.WIFI, Default::default())
        //   .expect("Failed to initialize Wi-Fi controller");
        let (wifi_controller, interfaces) =
            esp_radio::wifi::new(peripherals.WIFI, ControllerConfig::default())
                .expect("Could not initialize wifi controller");
        let wifi_interface = interfaces.station;

        // init network _stack (with dhcp)
        static STACK_RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
        let stack_resources = STACK_RESOURCES.init_with(|| StackResources::<5>::new());

        let mut trng_buf = [0u8; 16];
        // let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1.reborrow());
        let trng = Trng::try_new().unwrap();
        trng.read(&mut trng_buf);

        let net_seed = trng.random() as u64 | ((trng.random() as u64) << 32);
        let config = embassy_net::Config::dhcpv4(DhcpConfig::default());
        let (_stack, runner) = embassy_net::new(
            wifi_interface,
            config,
            stack_resources,
            net_seed
        );

        //
        //  Spawn tasks (Telemetry and radio connections)
        //

        spawner.spawn(net_task(runner).expect("Error in net_task"));
        spawner.spawn(
            wifi_connection_task(wifi_controller, SSID, PASSWORD)
                .expect("Error in wifi_connection"),
        );
        spawner.spawn(
            telemetry_task(
                SENSOR_CH.dyn_subscriber().unwrap(),
                TELEMETRY_CH.dyn_publisher().unwrap(),
            )
            .expect("Error in telemetry"),
        );

        // HTTP (DISABLE)
        #[cfg(all(feature = "http-api", feature = "sensors"))]
        spawner.spawn(smartpot_iot::tasks::wifi::http_api_task(
            _stack,
            dtec.dyn_subscriber().unwrap(),
        ));

        // MQTT
        #[cfg(all(feature = "mqtt", feature = "actuators", feature = "sensors"))]
        spawner.spawn(smartpot_iot::tasks::mqtt::mqtt_task(
            _stack,
            trng.clone(),
            TELEMETRY_CH.dyn_subscriber().unwrap(),
            COMMAND_CH.dyn_publisher().unwrap()
        ).expect("Error Running MQTT task"));
    }

    loop {
        /*
        info!("Free heap: {} bytes", esp_alloc::HEAP.free());
        info!("Used: {} bytes", esp_alloc::HEAP.used());
        info!("stats: {} ", esp_alloc::HEAP.stats());
        embassy_time::Timer::after(embassy_time::Duration::from_secs(5)).await;
        */
        core::future::pending::<()>().await;
    }
}
