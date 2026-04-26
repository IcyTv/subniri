mod device;

use std::cell::RefCell;
use std::rc::Rc;

use astal_wp::prelude::NodeExt;
use astal_wp::{Audio, Endpoint, MediaClass, Node, Wp};
use device::DeviceWidget;
use glib::{GString, Properties, clone};
use gtk4::CompositeTemplate;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use icons::Icon;

glib::wrapper! {
	pub struct VolumePopup(ObjectSubclass<imp::VolumePopup>)
		@extends gtk4::Popover, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Buildable, gtk4::Constraint, gtk4::ConstraintTarget, gtk4::ShortcutManager, gtk4::Native;
}

impl VolumePopup {
	pub fn new() -> Self {
		glib::Object::builder()
			.property("speaker-icon", Icon::Volume2.name())
			.property("mic-icon", Icon::Mic.name())
			.build()
	}
}

mod imp {
	use super::*;

	#[derive(Default, Properties, CompositeTemplate)]
	#[template(file = "./src/popups/volume/volume.blp")]
	#[properties(wrapper_type = super::VolumePopup)]
	pub struct VolumePopup {
		#[property(get, set)]
		speaker_icon: RefCell<String>,
		#[property(get, set)]
		default_speaker: RefCell<Option<Endpoint>>,
		#[property(get, set)]
		speaker_volume_percentage: RefCell<u8>,

		#[property(get, set)]
		mic_icon: RefCell<String>,
		#[property(get, set)]
		default_mic: RefCell<Option<Endpoint>>,
		#[property(get, set)]
		mic_volume_percentage: RefCell<u8>,

		#[template_child]
		devices_list_view: gtk4::TemplateChild<gtk4::ListView>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for VolumePopup {
		type ParentType = gtk4::Popover;
		type Type = super::VolumePopup;

		const NAME: &'static str = "VolumePopup";

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
			klass.bind_template_callbacks();
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for VolumePopup {
		fn constructed(&self) {
			self.parent_constructed();

			let wp = Wp::default();
			let audio = wp.audio();

			let obj = self.obj();

			audio
				.bind_property("default-speaker", &*obj, "default-speaker")
				.sync_create()
				.build();
			audio
				.bind_property("default-microphone", &*obj, "default-mic")
				.sync_create()
				.build();

			self.bind_speaker_icon(&audio);
			self.bind_speaker_volume(&audio);

			self.bind_mic_icon(&audio);
			self.bind_mic_volume(&audio);

			// Set up ListView for all audio devices
			let store = gtk4::gio::ListStore::new::<DeviceWidget>();

			// Create factories for list items and headers
			let factory = Self::create_device_factory();
			let header_factory = Self::create_header_factory();

			// Create sorter to group by device direction
			let sorter = gtk4::CustomSorter::new(move |a, b| {
				let a = a.downcast_ref::<DeviceWidget>().unwrap();
				let b = b.downcast_ref::<DeviceWidget>().unwrap();

				let a_endpoint = a.endpoint();
				let b_endpoint = b.endpoint();

				if let (Some(a_ep), Some(b_ep)) = (a_endpoint, b_endpoint) {
					// Sort by direction first (Output before Input)
					let a_dir = a_ep.media_class();
					let b_dir = b_ep.media_class();

					if a_dir != b_dir {
						return b_dir.cmp(&a_dir).into();
					}
				}

				gtk4::Ordering::Equal
			});

			let sort_model = gtk4::SortListModel::builder()
				.model(&store)
				.sorter(&sorter)
				.section_sorter(&sorter)
				.build();

			let selection_model = gtk4::NoSelection::new(Some(sort_model));

			self.devices_list_view.set_model(Some(&selection_model));
			self.devices_list_view.set_factory(Some(&factory));
			self.devices_list_view.set_header_factory(Some(&header_factory));

			// Populate devices from wp.nodes()
			let populate_devices = clone!(
				#[weak]
				store,
				move |wp: &Wp| {
					// Clear existing
					store.remove_all();

					// Add all audio nodes
					for node in wp.nodes().iter() {
						let media_class = node.media_class();

						// Filter for audio sink (speakers) and source (microphones) nodes only
						if matches!(media_class, MediaClass::AudioSink | MediaClass::AudioSource) {
							// Node IS an Endpoint, just downcast
							if let Ok(endpoint) = node.clone().downcast::<Endpoint>() {
								let widget = DeviceWidget::new(&endpoint);
								store.append(&widget);
							}
						}
					}
				}
			);

			populate_devices(&wp);
			wp.connect_nodes_notify(populate_devices);
		}
	}

	impl WidgetImpl for VolumePopup {}
	impl PopoverImpl for VolumePopup {}

	#[gtk4::template_callbacks]
	impl VolumePopup {
		#[template_callback]
		fn format_percentage(&self, percentage: u8) -> glib::GString {
			format!("{}%", percentage).into()
		}

		#[template_callback]
		fn on_toggle_speaker_mute(&self) {
			if let Some(speaker) = &*self.default_speaker.borrow() {
				speaker.set_mute(!speaker.is_muted());
			}
		}

		#[template_callback]
		fn on_toggle_mic_mute(&self) {
			if let Some(mic) = &*self.default_mic.borrow() {
				mic.set_mute(!mic.is_muted());
			}
		}
	}

	impl VolumePopup {
		fn create_device_factory() -> gtk4::SignalListItemFactory {
			let factory = gtk4::SignalListItemFactory::new();

			factory.connect_setup(|_, item| {
				let _item = item.downcast_ref::<gtk4::ListItem>().unwrap();
				// We'll set the actual DeviceWidget in bind
			});

			factory.connect_bind(|_, item| {
				let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
				if let Some(device_widget) = item.item().and_downcast::<DeviceWidget>() {
					item.set_child(Some(&device_widget));
				}
			});

			factory.connect_unbind(|_, item| {
				let item = item.downcast_ref::<gtk4::ListItem>().unwrap();
				item.set_child(None::<&gtk4::Widget>);
			});

			factory
		}

		fn create_header_factory() -> gtk4::SignalListItemFactory {
			let factory = gtk4::SignalListItemFactory::new();

			factory.connect_setup(|_, item| {
				let header = item.downcast_ref::<gtk4::ListHeader>().unwrap();
				let label = gtk4::Label::builder().halign(gtk4::Align::Start).build();
				header.set_child(Some(&label));
			});

			factory.connect_bind(|_, item| {
				let header = item.downcast_ref::<gtk4::ListHeader>().unwrap();
				let label = header.child().and_downcast::<gtk4::Label>().unwrap();

				if let Some(device_widget) = header.item().and_downcast::<DeviceWidget>()
					&& let Some(endpoint) = device_widget.endpoint()
				{
					let media_class = endpoint.media_class();
					let header_text = if media_class == MediaClass::AudioSink {
						label.add_css_class("playback-devices-label");
						"Playback Devices"
					} else {
						label.add_css_class("input-devices-label");
						"Input Devices"
					};
					label.set_text(header_text);
				}
			});

			factory.connect_unbind(|_, item| {
				let header = item.downcast_ref::<gtk4::ListHeader>().unwrap();
				if let Some(label) = header.child().and_downcast::<gtk4::Label>() {
					label.remove_css_class("playback-devices-label");
					label.remove_css_class("input-devices-label");
				}
			});

			factory
		}

		fn bind_speaker_icon(&self, audio: &Audio) {
			let obj = self.obj();

			let default_speaker =
				gtk4::PropertyExpression::new(Audio::static_type(), gtk4::Expression::NONE, "default-speaker");
			let default_speaker_mute =
				gtk4::PropertyExpression::new(Node::static_type(), Some(default_speaker.clone()), "mute");
			let default_speaker_volume =
				gtk4::PropertyExpression::new(Node::static_type(), Some(default_speaker), "volume");

			let speaker_icon = gtk4::ClosureExpression::new::<GString>(
				[&default_speaker_mute, &default_speaker_volume],
				glib::closure!(move |_: &glib::Object, is_muted: bool, volume: f64| -> GString {
					if is_muted {
						Icon::VolumeOff.name().into()
					} else {
						match volume {
							0.0 => Icon::VolumeX.name().into(),
							0.0..=0.2 => Icon::Volume.name().into(),
							0.2..=0.7 => Icon::Volume1.name().into(),
							0.7..=1.0 => Icon::Volume2.name().into(),
							_ => Icon::Volume2.name().into(),
						}
					}
				}),
			);
			speaker_icon.bind(&*obj, "speaker-icon", Some(audio));
		}

		fn bind_speaker_volume(&self, audio: &Audio) {
			let obj = self.obj();

			let speaker_binding = Rc::new(RefCell::new(None::<glib::Binding>));

			let bind_default_speaker_volume = clone!(
				#[strong]
				speaker_binding,
				#[weak]
				obj,
				move |audio: &Audio| {
					if let Some(b) = speaker_binding.borrow_mut().take() {
						b.unbind();
					}
					let speaker = audio.default_speaker();
					let binding = speaker
						.bind_property("volume", &obj, "speaker-volume-percentage")
						.transform_to(|_, volume: f64| Some((volume * 100.0) as u8))
						.transform_from(|_, volume_percentage: u8| Some((volume_percentage as f64) / 100.0))
						.bidirectional()
						.build();
					*speaker_binding.borrow_mut() = Some(binding);
				}
			);

			bind_default_speaker_volume(audio);
			audio.connect_default_speaker_notify(bind_default_speaker_volume);
		}

		fn bind_mic_icon(&self, audio: &Audio) {
			let obj = self.obj();

			let default_mic =
				gtk4::PropertyExpression::new(Audio::static_type(), gtk4::Expression::NONE, "default-microphone");
			let default_mic_mute = gtk4::PropertyExpression::new(Node::static_type(), Some(default_mic), "mute");
			let mic_icon = gtk4::ClosureExpression::new::<GString>(
				[&default_mic_mute],
				glib::closure!(move |_: &glib::Object, is_muted: bool| -> GString {
					if is_muted {
						Icon::MicOff.name().into()
					} else {
						Icon::Mic.name().into()
					}
				}),
			);

			mic_icon.bind(&*obj, "mic-icon", Some(audio));
		}

		fn bind_mic_volume(&self, audio: &Audio) {
			let obj = self.obj();
			let mic_binding = Rc::new(RefCell::new(None::<glib::Binding>));

			let bind_default_mic_volume = clone!(
				#[weak]
				obj,
				#[strong]
				mic_binding,
				move |audio: &Audio| {
					if let Some(b) = mic_binding.borrow_mut().take() {
						b.unbind();
					}
					let mic = audio.default_microphone();
					let binding = mic
						.bind_property("volume", &obj, "mic-volume-percentage")
						.transform_to(|_, volume: f64| Some((volume * 100.0) as u8))
						.transform_from(|_, volume_percentage: u8| Some((volume_percentage as f64) / 100.0))
						.bidirectional()
						.build();
					*mic_binding.borrow_mut() = Some(binding);
				}
			);

			bind_default_mic_volume(audio);
			audio.connect_default_microphone_notify(bind_default_mic_volume);
		}
	}
}
