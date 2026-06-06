use std::time::Instant;

use config::{ConfigFile, KdlDocument};
use iced::{
	Alignment, Background, Border, Element, Font, Length, Subscription, Task, Theme, font,
	futures::StreamExt,
	theme,
	widget::{column, container, row, space, text},
};
use neo_widgets::{
	style::COLORS,
	widgets::{neo_button, neo_card},
};

use crate::setting::Tab;

mod setting;

fn main() -> Result<(), iced::Error> {
	let _ = pretty_env_logger::try_init();

	let app = iced::application(Settings::new, Settings::update, Settings::view)
		.style(Settings::style)
		.subscription(Settings::subscription);

	app.run()
}

#[derive(Debug, Clone)]
enum Message {
	SelectSetting(usize),
	Setting(usize, setting::Message),
	Redraw,
	ConfigUpdated,
	Noop,
}

struct Settings {
	selected_setting: usize,
	tabs: Vec<Tab>,
	config: ConfigFile,
	doc: KdlDocument,
}

impl Settings {
	fn new() -> Self {
		let mut first = Tab::nightlight();
		first.selected.go_mut(true, Instant::now());

		let (doc, config) = ConfigFile::load().unwrap();

		Self {
			selected_setting: 0,
			tabs: vec![
				first,
				Tab::spotify(),
				Tab::homeassistant(),
				Tab::more_soon(),
			],
			config,
			doc,
		}
	}

	fn subscription(&self) -> Subscription<Message> {
		let config_watch = Subscription::run(|| {
			ConfigFile::watch().unwrap().map(|res| match res {
				Ok(()) => Message::ConfigUpdated,
				Err(e) => {
					log::warn!("Invalid config file: {e}");
					Message::Noop
				}
			})
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

				Task::none()
			}
			Message::SelectSetting(index) => {
				self.select(index);
				Task::none()
			}
			Message::Redraw => iced_runtime::task::effect(iced_runtime::Action::Window(
				iced::window::Action::RedrawAll,
			)),
			Message::Setting(index, msg) => self.tabs[index]
				.update(&mut self.config, &mut self.doc, msg)
				.map(move |msg| Message::Setting(index, msg)),
			Message::Noop => Task::none(),
		}
	}

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

		let mut content = row![
			neo_card(sidebar)
				.width(260)
				.height(Length::Fill)
				.padding(14)
				.background(COLORS.body),
		]
		.spacing(18)
		.padding(18);

		if let Some(setting) = self.tabs.get(self.selected_setting) {
			content = content.push(
				setting
					.view(&self.config)
					.map(|msg| Message::Setting(self.selected_setting, msg)),
			);
		}

		content.into()
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
}
