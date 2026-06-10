use iced::{
	Rectangle, Task, Vector,
	advanced::widget::{Operation, operation},
	widget::{Id, operation::AbsoluteOffset},
};
use iced_runtime::futures::MaybeSend;

pub fn follow_focus<M: MaybeSend + 'static>(id: Id) -> Task<M> {
	iced::advanced::widget::operate(FindFocusedInScrollable::new(id.clone()))
		.then(move |offset| iced::widget::operation::scroll_by(id.clone(), offset))
}

struct FindFocusedInScrollable {
	target: Id,
	scrollable: Option<ScrollableInfo>,
	focused: Option<Rectangle>,
	padding: f32,
}

impl FindFocusedInScrollable {
	fn new(target: Id) -> Self {
		Self {
			target,
			scrollable: None,
			focused: None,
			padding: 0.0,
		}
	}
}

#[derive(Clone, Copy)]
struct ScrollableInfo {
	bounds: Rectangle,
	_content_bounds: Rectangle,
	translation: Vector,
}

impl Operation<AbsoluteOffset> for FindFocusedInScrollable {
	fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<AbsoluteOffset>)) {
		operate(self);
	}

	fn scrollable(
		&mut self, id: Option<&Id>, bounds: Rectangle, content_bounds: Rectangle,
		translation: Vector, _state: &mut dyn operation::Scrollable,
	) {
		if id == Some(&self.target) {
			self.scrollable = Some(ScrollableInfo {
				bounds,
				_content_bounds: content_bounds,
				translation,
			});
		}
	}

	fn focusable(
		&mut self, _id: Option<&Id>, bounds: Rectangle, state: &mut dyn operation::Focusable,
	) {
		if state.is_focused() {
			self.focused = Some(bounds);
		}
	}

	fn finish(&self) -> operation::Outcome<AbsoluteOffset> {
		let Some(scrollable) = self.scrollable else {
			return operation::Outcome::None;
		};

		let Some(focused) = self.focused else {
			return operation::Outcome::None;
		};

		let visible = Rectangle {
			x: scrollable.bounds.x + scrollable.translation.x,
			y: scrollable.bounds.y + scrollable.translation.y,
			width: scrollable.bounds.width,
			height: scrollable.bounds.height,
		};

		let padding = self.padding;

		let mut dx = 0.0;
		let mut dy = 0.0;

		if focused.x < visible.x + padding {
			dx = focused.x - visible.x - padding;
		} else if focused.x + focused.width > visible.x + visible.width - padding {
			dx = focused.x + focused.width - (visible.x + visible.width) + padding;
		}

		if focused.y < visible.y + padding {
			dy = focused.y - visible.y - padding;
		} else if focused.y + focused.height > visible.y + visible.height - padding {
			dy = focused.y + focused.height - (visible.y + visible.height) + padding;
		}

		if dx == 0.0 && dy == 0.0 {
			operation::Outcome::None
		} else {
			operation::Outcome::Some(AbsoluteOffset { x: dx, y: dy })
		}
	}
}
