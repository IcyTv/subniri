use glib::object::ObjectBuilder;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

glib::wrapper! {
	pub struct Backdrop(ObjectSubclass<imp::Backdrop>)
		@extends gtk4::Widget, gtk4::Box,
		@implements gtk4::Accessible, gtk4::Orientable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl Backdrop {
	pub fn builder() -> BackdropBuilder {
		BackdropBuilder(glib::Object::builder::<Backdrop>())
	}
}

#[must_use]
pub struct BackdropBuilder(ObjectBuilder<'static, Backdrop>);

impl BackdropBuilder {
	pub fn orientation(self, orientation: gtk4::Orientation) -> Self {
		Self(self.0.property("orientation", orientation))
	}

	pub fn hexpand(self, hexpand: bool) -> Self {
		Self(self.0.property("hexpand", hexpand))
	}

	pub fn vexpand(self, vexpand: bool) -> Self {
		Self(self.0.property("vexpand", vexpand))
	}

	pub fn valign(self, align: gtk4::Align) -> Self {
		Self(self.0.property("valign", align))
	}

	pub fn halign(self, align: gtk4::Align) -> Self {
		Self(self.0.property("halign", align))
	}

	pub fn css_classes(self, classes: impl Into<glib::StrV>) -> Self {
		Self(self.0.property("css-classes", classes.into()))
	}

	pub fn build(self) -> Backdrop {
		self.0.build()
	}
}

mod imp {
	use super::*;

	#[derive(Default)]
	pub struct Backdrop {}

	#[glib::object_subclass]
	impl ObjectSubclass for Backdrop {
		type ParentType = gtk4::Box;
		type Type = super::Backdrop;
		const NAME: &'static str = "Backdrop";
	}

	impl ObjectImpl for Backdrop {}

	impl WidgetImpl for Backdrop {
		fn snapshot(&self, snapshot: &gtk4::Snapshot) {
			snapshot.push_blur(10.);
			snapshot.pop();

			self.parent_snapshot(snapshot);
		}
	}

	impl BoxImpl for Backdrop {}
}
