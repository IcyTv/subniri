use super::{NeoButton, neo_button, neo_toggle};

pub fn neo_toggle_button<'a, Message, Theme, Renderer>(
	toggled: bool,
) -> NeoButton<'a, Message, Theme, Renderer>
where
	Message: Clone + 'a,
	Theme: 'a,
	Renderer: iced::advanced::Renderer,
{
	neo_button(
		neo_toggle()
			.toggled(toggled)
			.track_style(super::NeoSurfaceStyle {
				shadow_width: 0.0,
				..Default::default()
			})
			.handle_style(super::NeoSurfaceStyle {
				shadow_width: 0.0,
				..Default::default()
			}),
	)
}
