use iced::{advanced::widget::Id, keyboard, window};

#[derive(Debug, Clone)]
pub(crate) enum Message {
	Prepared(window::Id, Option<Id>),
	Respond(window::Id, bool),
	Drag(window::Id),
	Key(window::Id, keyboard::Event),
	Deleted(window::Id, Result<(), String>),
}

impl From<Message> for crate::Message {
	fn from(message: Message) -> Self {
		Self::Dialog(message)
	}
}
