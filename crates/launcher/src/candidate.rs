use gtk4::gio::{Icon, ThemedIcon};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use launcher_common::{Candidate, IconRef, SectionHint};

glib::wrapper! {
	pub struct CandidateObject(ObjectSubclass<imp::CandidateObject>);
}

impl Default for CandidateObject {
	fn default() -> Self {
		glib::Object::builder().build()
	}
}

impl CandidateObject {
	pub fn new(value: Candidate) -> Self {
		glib::Object::builder()
			.property("provider", value.provider.0)
			.property("provider-score", value.provider_score)
			.property("id", value.id.0.as_ref())
			.property("activation", value.activation.0.as_ref())
			.property("title", value.title.as_ref())
			.property("subtitle", value.subtitle.map(|v| v.to_string()))
			.property("right_text", value.right_text.map(|v| v.to_string()))
			.property("icon", value.icon.and_then(|ir| to_gicon(ir).ok()))
			.property("kind", value.kind)
			.property("section_hint", value.section_hint.unwrap_or(SectionHint::None))
			.property("match_kind", value.match_kind)
			.build()
	}
}

fn to_gicon(icon_ref: IconRef) -> anyhow::Result<Icon> {
	match icon_ref {
		IconRef::SerializedIcon(serialized_icon) => Ok(Icon::for_string(&serialized_icon)?),
		// NOTE: unwrap() is safe here, because gtk guarantees that a GThemedIcon is a subclass of
		// GIcon
		IconRef::ThemedName(name) => Ok(ThemedIcon::new(&name).upcast::<Icon>()),
		_ => todo!("Convert IconRef"),
	}
}

mod imp {
	use std::cell::{Cell, RefCell};

	use glib::Properties;
	use launcher_common::{CandidateKind, MatchKind, SectionHint};

	use super::*;

	#[derive(Properties, Default)]
	#[properties(wrapper_type = super::CandidateObject)]
	pub struct CandidateObject {
		#[property(get, construct_only)]
		provider: RefCell<String>,
		#[property(get, construct_only)]
		provider_score: Cell<f32>,
		#[property(get, construct_only)]
		id: RefCell<String>,
		#[property(get, construct_only)]
		activation: RefCell<String>,

		#[property(get, set)]
		title: RefCell<String>,
		#[property(get, set)]
		subtitle: RefCell<Option<String>>,
		#[property(get, set)]
		right_text: RefCell<Option<String>>,
		#[property(get, set)]
		icon: RefCell<Option<Icon>>,

		#[property(get, set, default)]
		kind: Cell<CandidateKind>,
		#[property(get, set, default)]
		section_hint: Cell<SectionHint>,
		#[property(get, set, default)]
		match_kind: Cell<MatchKind>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for CandidateObject {
		type ParentType = glib::Object;
		type Type = super::CandidateObject;

		const NAME: &'static str = "CandidateObject";
	}

	#[glib::derived_properties]
	impl ObjectImpl for CandidateObject {
		fn constructed(&self) {
			self.parent_constructed();
		}
	}
}
