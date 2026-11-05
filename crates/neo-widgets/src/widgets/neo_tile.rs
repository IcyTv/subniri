//! Shared shape-led content for quick actions. Widgets retain ownership of custom layouts.
use super::{NeoButton, neo_button, neo_toggle};
use crate::style::COLORS;
use iced::{
	Font, Length, Size,
	alignment::Vertical,
	font,
	widget::{column, container, row, svg, text},
};
use std::borrow::Cow;

pub fn neo_tile<'a, Message: Clone + 'a>(
	icon: svg::Handle, title: impl Into<Cow<'a, str>>, subtitle: impl Into<Cow<'a, str>>,
	toggled: Option<bool>, icon_color: Option<iced::Color>, size: Size,
) -> NeoButton<'a, Message> {
	let title = title.into();
	let subtitle = subtitle.into();
	let inset = super::NeoContentSurfaceStyle::default().inset();
	let landscape = size.width > size.height * 1.25;
	let mini = size.width < 80.0 && size.height < 80.0;
	let compact = landscape && size.width < 200.0;
	let icon_space = if landscape || mini { 24.0 } else { 32.0 };
	let switch_space = if toggled.is_some() && !compact {
		40.0
	} else {
		0.0
	};
	let text_space =
		size.width - inset.width - icon_space - if compact { 8.0 } else { 28.0 } - switch_space;
	let room_for_text = landscape && text_space >= 40.0;
	let icon = container(svg(icon).width(icon_space).height(icon_space))
		.padding(if compact { 0.0 } else { 8.0 })
		.style(move |_| container::Style {
			background: if mini || compact {
				None
			} else {
				icon_color.map(Into::into)
			},
			border: iced::Border {
				color: COLORS.black,
				width: if mini || compact { 0.0 } else { 2.0 },
				radius: 3.0.into(),
			},
			..Default::default()
		});
	let content: iced::Element<'a, Message> = if landscape {
		let mut line = row![icon].spacing(8).align_y(Vertical::Center);
		if room_for_text {
			let mut labels = column![
				text(title)
					.size(16)
					.font(Font {
						weight: font::Weight::Bold,
						..Default::default()
					})
					.color(COLORS.text)
					.ellipsis(iced::advanced::text::Ellipsis::End)
					.wrapping(iced::widget::text::Wrapping::None)
			]
			.spacing(4);
			if !compact && text_space >= 120.0 {
				labels = labels.push(
					text(subtitle)
						.size(12)
						.color(COLORS.text)
						.ellipsis(iced::advanced::text::Ellipsis::End)
						.wrapping(iced::widget::text::Wrapping::None),
				);
			}
			line = line.push(labels.width(Length::Fill));
		}
		if let Some(toggled) = toggled.filter(|_| !compact) {
			line = line.push(
				neo_toggle::<Message>()
					.toggled(toggled)
					.width(32)
					.height(16),
			);
		}
		container(line).center(Length::Fill).into()
	} else {
		if mini {
			container(icon).center(Length::Fill).into()
		} else {
			container(
				column![
					icon,
					text(title)
						.size(16)
						.font(Font {
							weight: font::Weight::Bold,
							..Default::default()
						})
						.ellipsis(iced::advanced::text::Ellipsis::End)
						.wrapping(iced::widget::text::Wrapping::None)
				]
				.spacing(8)
				.align_x(iced::alignment::Horizontal::Center),
			)
			.center(Length::Fill)
			.into()
		}
	};
	neo_button(super::scale_to_fit(
		content,
		Size::new(
			(size.width - inset.width).max(1.0),
			(size.height - inset.height).max(1.0),
		),
	))
	.background(if toggled == Some(true) {
		COLORS.decorative.green90
	} else {
		COLORS.white
	})
	.width(Length::Fill)
	.height(Length::Fill)
}
