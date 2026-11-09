use super::{TagChoice, TagId, TagStore};
#[derive(Debug, Clone)]
pub enum Message {
	TagsLoaded(Result<TagStore, String>),
	RetryTags,
	TagFilter(Option<TagId>),
	EditTag(Option<TagId>),
	TagName(String),
	TagParent(TagChoice),
	SaveTag,
	CancelTag,
	AssignTag(TagChoice),
	RemoveTag(TagId),
	TagsSaved(Result<TagStore, String>),
}
impl From<Message> for crate::Message {
	fn from(message: Message) -> Self {
		Self::Tags(message)
	}
}
