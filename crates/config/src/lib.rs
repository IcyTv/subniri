use config_macros::{Config, ConfigFile, ConfigFileSerialize, ConfigSerialize};
use jiff::civil::Time;

/// This is the main configuration file for the subniri desktop shell.
/// If you want to, you can edit settings from here, and they'll be automatically syncronized to
/// all components in the subniri shell.
/// You can also edit the settings in a graphical interface (`snowconf`), and they will be
/// synchronized to this file.
#[derive(Default, Debug, ConfigFile, ConfigFileSerialize, garde::Validate)]
#[garde(allow_unvalidated)]
pub struct ConfigFile {
    /// Configuration for controlling the behavior of the nightlight.
    /// Temperature controls the color temperature (like with a light bulb) in [K]elvin. Lower means
    /// more red.
    /// Brightness controls the brighness of the screen. You probably won't want to go below ~0.2,
    /// because it becomes very hard to see.
    #[config(default)]
    #[garde(dive)]
    nightlight: Nightlight,
    /// Settings for controlling homeassistant.
    /// If you don't know what homeassistant is, please ignore this configuration. It's not for you.
    /// Otherwise, you'll need to set a url, and then log in in the Settings app.
    /// The connection keys are stored in your system's secret store.
    #[config(default)]
    #[garde(dive)]
    homeassistant: Homeassistant,
    /// Settings for controlling Spotify.
    /// Enabling this will let you do things, like adding a song to your library from the bar, at
    /// the cost of some networking overhead.
    /// To connect to spotify, you'll need a client id and a client secret by creating an
    /// application at <https://developer.spotify.com/dashboard> and setting the client id and
    /// client secret in the settings (Or manually adding them to the key store). Then you need to
    /// log in to your Spotify account.
    /// As far as I undertand, this will only work using a Spotify Premium account.
    /// Connection details and secrets are stored in your system's secret store.
    #[config(default)]
    #[garde(dive)]
    spotify: Spotify,
    /// Settings for controlling the System Menu (the bar module with the four squares)
    #[config(default)]
    #[garde(dive)]
    system_menu: SystemMenu,
}

#[derive(Debug, Config, ConfigSerialize, garde::Validate)]
#[garde(allow_unvalidated)]
pub struct Nightlight {
    /// If the nightlight integration should be enabled.
    enabled: bool,

    /// Whether you want to use a location provider (geoclue2) and a weather API to determine the dawn
    /// and dusk times based of the real times that the sun sets.
    /// This setting can't be combined with manually setting `dawn` or `dusk`
    #[config(default = false)]
    use_location: bool,

    /// Time (usually in the morning), when the nightlight should change to the `day` settings.
    /// The format can be anything close to `"HH:MM"`
    #[config(default = Time::new(7, 0, 0, 0).unwrap())]
    dawn: Time,
    /// Time (usually in the evening), when the nightlight should change to the `night` settings.
    /// The format can be anything close to `"HH:MM"`
    #[config(default = Time::new(20, 0, 0, 0).unwrap())]
    dusk: Time,

    /// Settings for daytime (after dawn, before dusk)
    #[config(default = NightlightSetting::day())]
    #[garde(dive)]
    day: NightlightSetting,
    /// Settings for night time (after dusk, before dawn)
    #[config(default = NightlightSetting::night())]
    #[garde(dive)]
    night: NightlightSetting,
}

impl Default for Nightlight {
    fn default() -> Self {
        Self {
            enabled: false,
            use_location: false,
            dawn: jiff::civil::Time::new(7, 0, 0, 0).unwrap(),
            dusk: jiff::civil::Time::new(20, 0, 0, 0).unwrap(),
            night: NightlightSetting::night(),
            day: NightlightSetting::day(),
        }
    }
}

#[derive(Debug, Config, ConfigSerialize, garde::Validate)]
pub struct NightlightSetting {
    /// Temperature of the light in [K]elvin. Basically the lower the number, the redder the light.
    /// Normal daytime temperature is 6500.
    /// Range [1000-10000]
    #[garde(range(min = 1000, max = 10000))]
    temperature: i32,
    /// Brightness of the light.
    /// Range [0.1-1.0]
    #[garde(range(min = 0.1, max = 1.0))]
    brightness: f64,
}

impl Default for NightlightSetting {
    fn default() -> Self {
        Self::day()
    }
}

impl NightlightSetting {
    pub const fn day() -> Self {
        Self {
            temperature: 6500,
            brightness: 1.0,
        }
    }

    pub const fn night() -> Self {
        Self {
            temperature: 2500,
            brightness: 0.7,
        }
    }
}

#[derive(Debug, Default, Config, ConfigSerialize, garde::Validate)]
#[garde(allow_unvalidated)]
pub struct Homeassistant {
    /// Should the homeassistant integration be enabled? If it's disabled, you won't be able to
    /// control your homeassistant-controlled devices from subniri.
    enabled: bool,
    /// The url of your homeassistant instance.
    /// Format "http://homeassistant.local:8123"
    url: Option<url::Url>,
    /// A list of device id's that you want to be able to control from subniri. If you go to the
    /// settings (`snowconf`), you'll be able to add all devices. But be aware, that this might
    /// require more resources from your system.
    #[config(list_style = children)]
    tracked_devices: Vec<String>,
}

#[derive(Debug, Default, Config, ConfigSerialize, garde::Validate)]
#[garde(allow_unvalidated)]
pub struct Spotify {
    /// Whether to enable the spotify integration or not.
    enabled: bool,
}

#[derive(Debug, Config)]
pub enum SystemMenuWidgets {
    Wifi,
    Bluetooth,
    Speaker,
    Microphone,
    Vpn,
    Nightlight,
}

#[derive(Debug, Default, Config, ConfigSerialize, garde::Validate)]
#[garde(allow_unvalidated)]
pub struct SystemMenu {
    /// Widgets to be displayed in the system menu. These put into 2 columns by the order they
    /// appear in this list.
    /// So: `a b c d`
    /// Turns into:
    ///     `a b`
    ///     `c d`
    widgets: Vec<SystemMenuWidgets>,
}

#[cfg(test)]
mod test {
    use super::*;
    use config_traits::{ConfigFileSerialize as _, ConfigFileValidateExt as _};

    #[test]
    fn test() {
        let doc = r#"
nightlight {
enabled
}

homeassistant {
}

spotify {
}
        "#;
        let res = ConfigFile::parse_validated(doc);

        let (mut doc, mut config) = match res {
            Ok(res) => res,
            Err(e) => {
                let report = miette::Report::new(e);
                panic!("{report:?}");
            }
        };

        println!("{:#?}", config);

        config
            .homeassistant
            .tracked_devices
            .extend(["foo", "bar", "baz"].into_iter().map(str::to_string));
        config.system_menu.widgets.push(SystemMenuWidgets::Wifi);
        config.system_menu.widgets.push(SystemMenuWidgets::Speaker);

        config.apply_to_kdl_document(&mut doc).unwrap();
        doc.autoformat();

        println!("\nRESULTING CONFIG:\n");

        println!("{}", doc.to_string());
    }

    #[test]
    fn validated_parse_rejects_out_of_range_nightlight_settings() {
        let doc = r#"
nightlight {
    day {
        temperature 500
        brightness 1.0
    }
}
        "#;

        let err = ConfigFile::parse_validated(doc).unwrap_err();

        assert!(matches!(err, config_traits::ConfigError::Validation { .. }));
    }
}
