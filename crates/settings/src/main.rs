use std::time::Instant;

use config::{ConfigFile, KdlDocument};
use iced::{
	Alignment, Background, Border, Element, Font, Length, Subscription, Task, Theme, font,
	futures::{StreamExt, stream},
	theme,
	widget::{column, container, row, space, text},
};
use neo_widgets::{
	style::COLORS,
	widgets::{neo_button, neo_card},
};

use crate::setting::Tab;

mod setting;

fn main() -> Result<(), Box<dyn std::error::Error>> {
	log::init!("settings", "snowconf")?;

	let app = iced::application(Settings::new, Settings::update, Settings::view)
		.style(Settings::style)
		.subscription(Settings::subscription);

	Ok(app.run()?)
}

#[derive(Debug, Clone)]
enum Message {
	SelectSetting(usize),
	Setting(usize, setting::Message),
	ClearConfigOverride,
	CopyNixChanges,
	Redraw,
	ConfigUpdated,
	Noop,
}

struct Settings {
	selected_setting: usize,
	tabs: Vec<Tab>,
	config: ConfigFile,
	declarative_config: ConfigFile,
	doc: KdlDocument,
}

impl Settings {
	fn new() -> (Self, Task<Message>) {
		let mut first = Tab::nightlight();
		first.selected.go_mut(true, Instant::now());

		let (doc, config) = match ConfigFile::load() {
			Ok(config) => config,
			Err(error) => {
				log::warn!("Error loading config, using defaults: {error}");
				(KdlDocument::new(), ConfigFile::default())
			}
		};
		let declarative_config = ConfigFile::load_declarative().map_or_else(
			|error| {
				log::warn!("Error loading declarative config: {error}");
				config.clone()
			},
			|(_, config)| config,
		);

		let settings = Self {
			selected_setting: 0,
			tabs: vec![
				first,
				Tab::spotify(),
				Tab::homeassistant(),
				Tab::more_soon(),
			],
			config,
			declarative_config,
			doc,
		};

		let tasks = settings.init_tabs();

		(settings, tasks)
	}

	fn subscription(&self) -> Subscription<Message> {
		let config_watch = Subscription::run(|| {
			ConfigFile::watch().map_or_else(
				|error| {
					log::warn!("Error watching config file: {error}");
					stream::once(async { Message::Noop }).boxed()
				},
				|watch| {
					watch
						.map(|res| match res {
							Ok(()) => Message::ConfigUpdated,
							Err(e) => {
								log::warn!("Invalid config file: {e}");
								Message::Noop
							}
						})
						.boxed()
				},
			)
		});

		let mut subs = vec![config_watch];

		let at = Instant::now();
		if self.tabs.iter().any(|setting| setting.is_animating(at)) {
			subs.push(iced::window::frames().map(|_| Message::Redraw));
		}

		Subscription::batch(subs)
	}

	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::ConfigUpdated => {
				let (doc, config) = match ConfigFile::load() {
					Ok(c) => c,
					Err(e) => {
						log::warn!("Error loading config: {e}");
						return Task::none();
					}
				};

				self.doc = doc;
				self.config = config;
				match ConfigFile::load_declarative() {
					Ok((_, config)) => self.declarative_config = config,
					Err(error) => log::warn!("Error loading declarative config: {error}"),
				}

				self.init_tabs()
			}
			Message::ClearConfigOverride => {
				match ConfigFile::clear_override() {
					Ok((doc, config)) => {
						self.doc = doc;
						self.declarative_config = config.clone();
						self.config = config;
					}
					Err(error) => log::error!("Failed to clear config override: {error}"),
				}
				self.init_tabs()
			}
			Message::CopyNixChanges => iced::clipboard::write(
				self.config.nix_diff(&self.declarative_config),
			)
			.map(|result| {
				if let Err(error) = result {
					log::warn!("Failed to copy Nix changes: {error:?}");
				}
				Message::Noop
			}),
			Message::SelectSetting(index) => {
				self.select(index);
				Task::none()
			}
			Message::Redraw => iced_runtime::task::effect(iced_runtime::Action::Window(
				iced::window::Action::RedrawAll,
			)),
			Message::Setting(index, msg) => {
				self.tabs.get_mut(index).map_or_else(Task::none, |tab| {
					tab.update(&mut self.config, &mut self.doc, msg)
						.map(move |msg| Message::Setting(index, msg))
				})
			}
			Message::Noop => Task::none(),
		}
	}

	#[allow(clippy::too_many_lines)]
	fn view(&self) -> Element<'_, Message> {
		let mut sidebar = column![
			column![
				text("SETTINGS").color(COLORS.text).size(28).font(Font {
					weight: font::Weight::Bold,
					..Default::default()
				}),
				text("break glass, tune pixels")
					.size(12)
					.color(COLORS.text.scale_alpha(0.7))
					.font(Font {
						weight: font::Weight::Bold,
						..Default::default()
					})
			]
			.spacing(2),
			container("")
				.width(Length::Fill)
				.height(2)
				.style(|_| container::Style {
					background: Some(Background::Color(COLORS.border)),
					..Default::default()
				}),
		]
		.spacing(14);

		let at = Instant::now();

		for (index, setting) in self.tabs.iter().enumerate() {
			let widget = neo_button(
				row![
					container(setting.icon())
						.width(36)
						.height(36)
						.align_y(Alignment::Center)
						.align_x(Alignment::Center)
						.padding(8)
						.style(move |_| container::Style {
							border: Border {
								width: 2.0,
								color: COLORS.border,
								radius: 3.into(),
							},
							background: Some(Background::Color(setting.icon_bg_color(at))),
							..Default::default()
						}),
					text(setting.name())
						.width(Length::Fill)
						.color(COLORS.text)
						.size(18)
						.font(Font {
							weight: font::Weight::Bold,
							..Default::default()
						})
						.ellipsis(text::Ellipsis::End)
				]
				.spacing(10)
				.align_y(Alignment::Center)
				.width(Length::Fill),
			)
			.background(setting.color(at))
			.shadow_width(setting.shadow_width(at))
			.width(Length::Fill)
			.on_press(Message::SelectSetting(index));

			sidebar = sidebar.push(widget);
		}

		sidebar = sidebar.push(space::vertical());

		let mut settings_row = row![
			neo_card(sidebar)
				.width(260)
				.height(Length::Fill)
				.padding(14)
				.background(COLORS.body),
		]
		.spacing(18)
		.padding(18);

		if let Some(setting) = self.tabs.get(self.selected_setting) {
			settings_row = settings_row.push(
				setting
					.view(&self.config)
					.map(|msg| Message::Setting(self.selected_setting, msg)),
			);
		}

		let mut content = column![].spacing(12);
		if ConfigFile::override_active() {
			let nix_diff = self.config.nix_diff(&self.declarative_config);
			let diff_text = if nix_diff.is_empty() {
				"The local override matches the declarative config.".to_string()
			} else {
				nix_diff
			};
			content = content.push(
				neo_card(
					column![
						text("LOCAL CONFIG OVERRIDE ACTIVE")
							.size(18)
							.weight(font::Weight::Bold),
						text("These live changes are not part of your Home Manager configuration. Apply the suggested Nix assignments to make them permanent.")
							.size(13)
							.wrapping(text::Wrapping::Word),
						container(text(diff_text).size(13).wrapping(text::Wrapping::Word))
							.width(Length::Fill)
							.padding(10)
							.style(|_| container::Style {
								background: Some(Background::Color(COLORS.white)),
								border: Border {
									color: COLORS.border,
									width: 2.0,
									radius: 3.into(),
								},
								..Default::default()
							}),
						row![
							neo_button(text("COPY NIX CHANGES").weight(font::Weight::Bold))
								.on_press(Message::CopyNixChanges),
							neo_button(text("DISCARD LOCAL CHANGES").weight(font::Weight::Bold))
								.on_press(Message::ClearConfigOverride),
						]
						.spacing(10),
					]
					.spacing(8),
				)
				.width(Length::Fill)
				.padding(14)
				.background(COLORS.decorative.yellow90),
			);
		}

		content.push(settings_row).into()
	}

	#[allow(clippy::unused_self)]
	fn style(&self, _theme: &Theme) -> theme::Style {
		theme::Style {
			background_color: COLORS.decorative.pink70,
			text_color: COLORS.text,
		}
	}

	fn select(&mut self, index: usize) {
		self.selected_setting = index;

		let now = Instant::now();

		for (i, item) in self.tabs.iter_mut().enumerate() {
			item.selected.go_mut(i == index, now);
		}
	}

	fn init_tabs(&self) -> Task<Message> {
		Task::batch(self.tabs.iter().enumerate().map(|(index, tab)| {
			tab.init(&self.config)
				.map(move |message| Message::Setting(index, message))
		}))
	}
}
