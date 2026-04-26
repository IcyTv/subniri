use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::candidate::CandidateObject;

glib::wrapper! {
	pub struct CandidateRow(ObjectSubclass<imp::CandidateRow>)
		@extends gtk4::Box, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Orientable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl CandidateRow {
	pub fn new() -> Self {
		glib::Object::builder().build()
	}

	pub fn set_candidate(&self, candidate_obj: &CandidateObject) {
		candidate_obj
			.bind_property("title", self, "title")
			.sync_create()
			.build();
		candidate_obj
			.bind_property("subtitle", self, "subtitle")
			.sync_create()
			.build();
		candidate_obj
			.bind_property("right_text", self, "right_text")
			.sync_create()
			.build();
	}
}

mod imp {
	use std::cell::RefCell;

	use glib::Properties;
	use gtk4::CompositeTemplate;

	use super::*;

	#[derive(CompositeTemplate, Properties, Default)]
	#[template(file = "./src/candidate_row.blp")]
	#[properties(wrapper_type = super::CandidateRow)]
	pub struct CandidateRow {
		#[property(get, set)]
		title: RefCell<String>,
		#[property(get, set)]
		subtitle: RefCell<Option<String>>,
		#[property(get, set)]
		right_text: RefCell<Option<String>>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for CandidateRow {
		type ParentType = gtk4::Box;
		type Type = super::CandidateRow;

		const NAME: &'static str = "CandidateRow";

		fn class_init(klass: &mut Self::Class) {
			Self::bind_template(klass);
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for CandidateRow {
		fn constructed(&self) {
			self.parent_constructed();
		}
	}

	impl WidgetImpl for CandidateRow {}
	impl BoxImpl for CandidateRow {}
}
