use std::cell::RefCell;

use astal_io::prelude::*;
use glib::{Properties, clone};
use gtk4::CompositeTemplate;
use gtk4::subclass::prelude::*;
use libgweather::{Info, Location, Provider, TemperatureUnit};

use crate::icons::Icon;

glib::wrapper! {
	pub struct WeatherDisplay(ObjectSubclass<imp::WeatherDisplay>)
		@extends gtk4::Box, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget;
}

impl WeatherDisplay {
	pub fn new() -> Self {
		glib::Object::builder()
			.property("weather-wind-icon", Icon::Wind.name())
			.property("weather-humidity-icon", Icon::Droplets.name())
			.property("temperature-icon", Icon::Thermometer.name())
			.build()
	}
}

mod imp {
	use libgweather::{ConditionPhenomenon, ConditionQualifier, Sky, SpeedUnit};

	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/clock/weather.blp")]
	#[properties(wrapper_type = super::WeatherDisplay)]
	pub struct WeatherDisplay {
		#[property(get, set)]
		weather_info: RefCell<Info>,

		#[property(get, set)]
		weather_icon: RefCell<String>,
		#[property(get, set)]
		temperature: RefCell<String>,
		#[property(get, set)]
		conditions: RefCell<String>,
		#[property(get, set)]
		wind_speed: RefCell<String>,
		#[property(get, set)]
		humidity: RefCell<String>,
		#[property(get, set)]
		apparent_temperature: RefCell<String>,

		// "Static" Icons
		#[property(get, set)]
		weather_wind_icon: RefCell<String>,
		#[property(get, set)]
		weather_humidity_icon: RefCell<String>,
		#[property(get, set)]
		temperature_icon: RefCell<String>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for WeatherDisplay {
		type ParentType = gtk4::Box;
		type Type = super::WeatherDisplay;

		const NAME: &'static str = "WeatherDisplay";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			klass.bind_template_callbacks();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for WeatherDisplay {
		fn constructed(&self) {
			self.parent_constructed();

			let world = Location::world().expect("to get world location");

			let info = self.weather_info.borrow();
			info.set_enabled_providers(Provider::MET_NO | Provider::METAR);

			let info2 = (*info).clone();
			glib::spawn_future_local(async move {
				let location = world.detect_nearest_city_future(49.450258, 8.478273).await;
				match location {
					Ok(loc) => {
						println!("Using weather location: {:?}", loc.name());
						info2.set_location(Some(&loc));
					}
					Err(e) => {
						eprintln!("Failed to detect nearest city: {}", e);
						let location = world.find_nearest_city(49.450258, 8.478273);
						info2.set_location(Some(&location));
					}
				}

				info2.update();
			});

			let obj = self.obj();
			let update_weather = clone!(
				#[weak]
				obj,
				move |info: &Info| {
					if !info.is_valid() {
						eprintln!("Weather info is not valid");
						return;
					}

					let icon_name = info.icon_name();
					println!("Weather icon name: {}", icon_name);
					obj.set_weather_icon(icon_name);
					let temp = info.value_temp(TemperatureUnit::Centigrade);
					if let Some(temp) = temp {
						let temp_str = format!("{:.1} °C", temp);
						obj.set_temperature(&*temp_str);
					} else {
						obj.set_temperature("N/A");
					}

					let conditions = obj.imp().conditions();
					println!("Weather conditions: {}", conditions);
					obj.set_conditions(conditions);

					let wind = info.value_wind(SpeedUnit::Ms);
					if let Some((speed, _)) = wind {
						obj.set_wind_speed(format!("{:.1} m/s", speed).as_str());
					}

					let humidity = info.humidity();
					obj.set_humidity(humidity);

					let apparent_temp = info.value_apparent(TemperatureUnit::Centigrade);
					if let Some(atemp) = apparent_temp {
						let precip_str = format!("{:.1} °C", atemp);
						obj.set_apparent_temperature(&*precip_str);
					} else {
						obj.set_apparent_temperature("N/A");
					}
				}
			);

			info.connect_updated(update_weather);
		}
	}

	impl WidgetImpl for WeatherDisplay {}
	impl BoxImpl for WeatherDisplay {}

	#[gtk4::template_callbacks]
	impl WeatherDisplay {
		#[template_callback]
		fn convert_icon(&self, glib_icon: &str) -> &'static str {
			match glib_icon {
				"weather-clear" => Icon::Sun.name(),
				"weather-clear-night" => Icon::Moon.name(),
				"weather-few-clouds" => Icon::CloudSun.name(),
				"weather-few-clouds-night" => Icon::CloudMoon.name(),
				"weather-showers-scattered" => Icon::CloudSunRain.name(),
				"weather-showers-scattered-night" => Icon::CloudMoonRain.name(),

				"weather-overcast" => Icon::Cloud.name(),
				"weather-fog" => Icon::CloudFog.name(),
				"weather-showers" => Icon::CloudDrizzle.name(),
				"weather-snow" => Icon::Snowflake.name(),
				"weather-storm" => Icon::Wind.name(),
				"weather-windy" => Icon::Wind.name(),
				"weather-sever-alert" => Icon::CloudAlert.name(),

				icon => {
					println!("Unknown weather icon: {}", icon);
					Icon::Cloud.name()
				}
			}
		}
	}

	impl WeatherDisplay {
		fn conditions(&self) -> String {
			let conditions = self.weather_info.borrow().value_conditions();

			match conditions {
				Some((ConditionPhenomenon::None | ConditionPhenomenon::Invalid, _)) | None => (),
				Some((phenomenon, qualifier)) => return Self::format_phenomenon(phenomenon, qualifier),
			}

			let sky_str = match self.weather_info.borrow().value_sky() {
				Some(Sky::Clear) => "Clear",
				Some(Sky::Broken) => "Broken Clouds",
				Some(Sky::Scattered) => "Scattered Clouds",
				Some(Sky::Few) => "Few Clouds",
				Some(Sky::Overcast) => "Overcast",
				Some(Sky::Last) => "Last Sky Data (why?)",
				Some(Sky::Invalid) => "Invalid Sky Data",
				None => "No Sky Data",
				_ => "Unknown",
			};

			format!("{}", sky_str)
		}

		fn format_phenomenon(phenomenon: ConditionPhenomenon, qualifier: ConditionQualifier) -> String {
			use ConditionPhenomenon::*;
			use ConditionQualifier::*;
			let p_str = match phenomenon {
				Drizzle => "Drizzle",
				Rain => "Rain",
				Snow => "Snow",
				SnowGrains => "Snow Grains",
				IceCrystals => "Ice Crystals",
				IcePellets => "Ice Pellets",
				Hail => "Hail",
				SmallHail => "Small Hail",
				Mist => "Mist",
				Fog => "Fog",
				Smoke => "Smoke",
				VolcanicAsh => "Volcanic Ash",
				Sand => "Sand",
				Haze => "Haze",
				Spray => "Spray",
				Dust => "Dust",
				Squall => "Squall",
				Sandstorm => "Sandstorm",
				Duststorm => "Duststorm",
				FunnelCloud => "Funnel Cloud",
				Tornado => "Tornado",
				DustWhirls => "Dust Whirls",
				_ => "Unknown",
			};

			match qualifier {
				Thunderstorm => format!("Thunderstorm with {}", p_str),
				Showers => format!("{} Showers", p_str),
				Vicinity => format!("{} nearby", p_str),

				Light => format!("Light {}", p_str),
				Heavy => format!("Heavy {}", p_str),
				Freezing => format!("Freezing {}", p_str),
				Blowing => format!("Blowing {}", p_str),
				Drifting => format!("Drifting {}", p_str),
				Shallow => format!("Shallow {}", p_str),
				Patches => format!("Patches of {}", p_str),
				Partial => format!("Partial {}", p_str),

				Moderate | ConditionQualifier::None | _ => p_str.to_string(),
			}
		}
	}
}
