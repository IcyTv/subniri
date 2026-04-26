use std::cell::RefCell;
use std::sync::{Arc, LazyLock};

use glib::{Properties, clone};
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{CompositeTemplate, glib};
use launcher_common::{PreviewModel, ProviderContext};

use crate::candidate::CandidateObject;
use crate::candidate_row::CandidateRow;

glib::wrapper! {
	pub struct LauncherWidget(ObjectSubclass<imp::LauncherWidget>)
		@extends gtk4::Box, gtk4::Widget,
		@implements gtk4::Accessible, gtk4::Orientable, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl LauncherWidget {
	pub fn new() -> Self {
		glib::Object::builder().property("session-id", 0u64).build()
	}

	pub fn focus_input(&self) {
		self.set_input_data("");

		let imp = self.imp();
		imp.input_field.grab_focus_without_selecting();
		let no_objects: &[glib::Object] = &[];
		imp.candidate_store.splice(0, imp.candidate_store.n_items(), no_objects);
	}

	pub fn new_session_id(&self) {
		self.set_session_id(self.session_id() + 1);
	}
}

mod imp {
	use std::{cell::Cell, cmp::Ordering};

	use app_provider::AppProvider;
	use calc_provider::CalcProvider;
	use futures::{FutureExt, StreamExt, future::join_all, stream::FuturesUnordered};
	use gtk4::{gdk, gio::ListStore};
	use launcher_common::{Provider, ProviderEvent, Query, Revision, RuntimeHandle, SessionHandle, SessionId};

	use super::*;

	static PROVIDERS: LazyLock<Arc<[Arc<dyn Provider>]>> = LazyLock::new(|| {
		Arc::<[Arc<dyn Provider>]>::from([Arc::new(CalcProvider::new()) as _, Arc::new(AppProvider::new()) as _])
	});

	#[derive(Properties, CompositeTemplate)]
	#[template(file = "./src/launcher.blp")]
	#[properties(wrapper_type = super::LauncherWidget)]
	pub struct LauncherWidget {
		#[property(get, set)]
		input_data: RefCell<String>,
		#[property(get, set)]
		session_id: Cell<u64>,
		#[property(get)]
		provider_names: RefCell<gtk4::StringList>,
		#[template_child]
		pub(super) input_field: TemplateChild<gtk4::Text>,
		#[template_child]
		preview_label: TemplateChild<gtk4::Label>,
		#[template_child]
		list_view: TemplateChild<gtk4::ListView>,
		#[template_child]
		results: TemplateChild<gtk4::ScrolledWindow>,

		revision: Cell<u64>,
		provider_ctx: Arc<LauncherContext>,
		rt: RuntimeHandle,

		pub(super) candidate_store: ListStore,
		sorted_model: gtk4::SortListModel,
		selection_model: gtk4::SelectionModel,
	}

	impl Default for LauncherWidget {
		fn default() -> Self {
			let rt = RuntimeHandle::new(
				move |job| {
					glib::MainContext::default().spawn(job);
				},
				move |job| {
					Box::pin(async move {
						gtk4::gio::spawn_blocking(job)
							.await
							.map_err(|_| anyhow::anyhow!("Failed to run blocking task..."))
					})
				},
			);

			let store = ListStore::new::<CandidateObject>();

			let sorter = gtk4::CustomSorter::new(|a, b| {
				let a = a.downcast_ref::<CandidateObject>().unwrap();
				let b = b.downcast_ref::<CandidateObject>().unwrap();

				let match_order = a.match_kind().priority().cmp(&b.match_kind().priority());
				if match_order != Ordering::Equal {
					return match_order.into();
				}

				b.provider_score()
					.partial_cmp(&a.provider_score())
					.unwrap_or(Ordering::Equal)
					.into()
			});

			let section_sorter = gtk4::CustomSorter::new(clone!(
				#[strong]
				sorter,
				move |a, b| {
					let a = a.downcast_ref::<CandidateObject>().unwrap();
					let b = b.downcast_ref::<CandidateObject>().unwrap();

					// // FIXME: This comparison isn't good.
					// // FIXME: Also: this sorts based on the provider FIRST, only then on the actual
					// // matches... That isn't great...
					// a.provider().cmp(&b.provider()).into()
					let provider_cmp = a.provider().cmp(&b.provider());
					if provider_cmp != Ordering::Equal {
						sorter.compare(a, b)
					} else {
						Ordering::Equal.into()
					}
				}
			));

			let sorted_model = gtk4::SortListModel::builder()
				.model(&store)
				.sorter(&sorter)
				.section_sorter(&section_sorter)
				.build();

			let selection_model = gtk4::SingleSelection::new(Some(sorted_model.clone()));

			let provider_names = PROVIDERS
				.iter()
				.map(|p| p.name().to_string())
				.collect::<gtk4::StringList>();

			Self {
				input_data: Default::default(),
				session_id: Default::default(),
				provider_names: RefCell::new(provider_names),
				input_field: Default::default(),
				preview_label: Default::default(),
				list_view: Default::default(),
				results: Default::default(),
				revision: Default::default(),
				provider_ctx: Default::default(),
				rt,
				candidate_store: store,
				sorted_model,
				selection_model: selection_model.into(),
			}
		}
	}

	#[glib::object_subclass]
	impl ObjectSubclass for LauncherWidget {
		type ParentType = gtk4::Box;
		type Type = super::LauncherWidget;

		const NAME: &'static str = "LauncherWidget";

		fn class_init(klass: &mut Self::Class) {
			Self::bind_template(klass);
			Self::bind_template_callbacks(klass);
		}

		fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
			obj.init_template();
		}
	}

	#[glib::derived_properties]
	impl ObjectImpl for LauncherWidget {
		fn constructed(&self) {
			self.parent_constructed();

			self.list_view.set_model(Some(&self.selection_model));
			self.list_view.set_factory(Some(&create_item_factory()));
			self.list_view.set_header_factory(Some(&create_header_factory()));

			let obj = self.obj();

			let key_controller = gtk4::EventControllerKey::new();
			key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);

			key_controller.connect_key_pressed(clone!(
				#[weak]
				obj,
				#[upgrade_or]
				glib::Propagation::Proceed,
				move |_, key, _, _| {
					let imp = obj.imp();
					let results_shown = imp.candidate_store.n_items() > 0;

					if results_shown {
						match key {
							gdk::Key::Down => {
								if imp.input_field.has_focus() {
									imp.list_view.grab_focus();
									return glib::Propagation::Stop;
								} else {
									return glib::Propagation::Proceed;
								}
							}
							gdk::Key::Up => {
								let selected_index = imp.selection_model.selection().minimum();
								let is_first_selected =
									selected_index == 0 || selected_index == gtk4::INVALID_LIST_POSITION;

								if let Some(focused) = obj.root().and_then(|r| r.focus()) {
									if focused.is_ancestor(&*imp.list_view) && is_first_selected {
										imp.input_field.grab_focus();
										imp.input_field
											.emit_move_cursor(gtk4::MovementStep::BufferEnds, 0, false);
										return glib::Propagation::Stop;
									}
								}
							}
							_ => {}
						}
					}

					glib::Propagation::Proceed
				}
			));

			obj.add_controller(key_controller.clone());

			let buffer = self.input_field.buffer();

			obj.bind_property("input-data", &buffer, "text")
				.bidirectional()
				.sync_create()
				.build();

			self.candidate_store.connect_items_changed(clone!(
				#[weak(rename_to = results)]
				self.results,
				move |s, _, _, _| {
					if s.n_items() == 0 {
						results.set_visible(false);
					} else {
						results.set_visible(true);
					}
				}
			));

			let provider_streams = PROVIDERS
				.iter()
				.map(|p| {
					let receiver = p.init(self.provider_ctx.clone(), self.rt.clone());
					let pid = p.id();
					receiver.map(move |init_result| (pid, init_result)).boxed()
				})
				.collect::<FuturesUnordered<_>>()
				.filter_map(|(pid, maybe_receiver)| async move {
					match maybe_receiver {
						Ok(receiver) => Some(receiver.map(move |ev| (pid, ev)).boxed()),
						Err(e) => {
							eprintln!("Provide {pid:?} failed to start: {e}");
							None
						}
					}
				})
				.flatten_unordered(None);
			let mut provider_streams = Box::pin(provider_streams);

			glib::spawn_future_local(clone!(
				#[weak]
				obj,
				async move {
					let imp = obj.imp();
					while let Some((provider_id, event)) = provider_streams.next().await {
						// println!("Event from {provider_id:?}: {event:?}");
						// TODO: Revision filtering?
						match event {
							ProviderEvent::CandidateUpsert(candidate) => {
								let id = candidate.id.0.clone();
								let cand = CandidateObject::new(candidate);

								let exists = imp.candidate_store.find_with_equal_func(|w| {
									let obj = w.downcast_ref::<CandidateObject>().unwrap();
									obj.id() == id.as_ref()
								});
								if let Some(index) = exists {
									imp.candidate_store.splice(index, 1, &[cand]);
								} else {
									imp.candidate_store.append(&cand)
								}
							}
							ProviderEvent::CandidateRemove { id } => {
								let index = imp.candidate_store.find_with_equal_func(|w| {
									let candidate = w.downcast_ref::<CandidateObject>().unwrap();
									candidate.id() == id.0.as_ref()
								});
								if let Some(index) = index {
									imp.candidate_store.remove(index);
								}
							}
							ProviderEvent::Reset => {
								let mut i = 0;
								while i < imp.candidate_store.n_items() {
									let item = imp.candidate_store.item(i).and_downcast::<CandidateObject>().unwrap();
									if item.provider() == provider_id.0 {
										imp.candidate_store.remove(i);
									} else {
										i += 1;
									}
								}
							}
							ProviderEvent::Status(_status) => {}
							ProviderEvent::PreviewUpdate(_preview) => {}
							ProviderEvent::Done => {}
						}
					}
				}
			));

			obj.connect_input_data_notify(move |obj| {
				let imp = obj.imp();
				let input = Arc::<str>::from(obj.input_data());
				let session_id = SessionId(imp.session_id.get());
				let cur_rev = imp.revision.get() + 1;
				imp.revision.set(cur_rev);
				let revision = Revision(cur_rev);

				let session = SessionHandle { session_id, revision };

				let query = Query { raw: input, cursor: 0 };

				glib::spawn_future_local(clone!(
					#[weak]
					obj,
					#[strong]
					query,
					async move {
						let imp = obj.imp();
						let provider_ctx = imp.provider_ctx.clone();
						let rt = imp.rt.clone();
						let update_futures = PROVIDERS
							.iter()
							.map(|p| p.update_query(session, query.clone(), provider_ctx.clone(), rt.clone()));
						join_all(update_futures).await;
					}
				));
			});

			self.selection_model.connect_items_changed(clone!(
				#[weak(rename_to = list_view)]
				self.list_view,
				move |model, _, _, _| {
					if model.n_items() > 0 {
						list_view.scroll_to(0, gtk4::ListScrollFlags::all(), None);
					}
				}
			));

			self.sorted_model.connect_items_changed(clone!(
				#[weak]
				buffer,
				#[weak(rename_to = label)]
				self.preview_label,
				move |store, _, _, _| {
					if let Some(preview_item) = store.item(0).and_downcast::<CandidateObject>() {
						let text = format!(
							"<span foreground=\"#00000000\" alpha=\"1\">{}</span><span foreground=\"gray\" size=\"xx-small\"> - {}</span>",
							buffer.text(),
							preview_item.title()
						);
						label.set_label(&text);
					} else {
						label.set_label("")
					}
				}
			));
		}
	}

	impl WidgetImpl for LauncherWidget {}
	impl BoxImpl for LauncherWidget {}

	#[gtk4::template_callbacks]
	impl LauncherWidget {}

	fn create_item_factory() -> gtk4::SignalListItemFactory {
		let factory = gtk4::SignalListItemFactory::new();

		factory.connect_setup(|_, li| {
			let li = li.downcast_ref::<gtk4::ListItem>().expect("to be a ListItem");
			let row = CandidateRow::new();
			li.set_child(Some(&row));
		});
		factory.connect_bind(|_, li| {
			let list_item = li.downcast_ref::<gtk4::ListItem>().expect("Needs to be a ListItem");
			let child = list_item.child().and_downcast::<CandidateRow>().unwrap();
			let item = list_item.item().and_downcast::<CandidateObject>().unwrap();

			child.set_candidate(&item);
		});
		factory.connect_unbind(|_, li| {
			let list_item = li.downcast_ref::<gtk4::ListItem>().expect("Needs to be a ListItem");
			// TODO: I think this is wrong...
			list_item.set_child(None::<&gtk4::Widget>);
		});
		factory
	}

	fn create_header_factory() -> gtk4::SignalListItemFactory {
		let factory = gtk4::SignalListItemFactory::new();

		factory.connect_setup(|_, li| {
			let lh = li.downcast_ref::<gtk4::ListHeader>().expect("to be a ListHeader");
			lh.set_child(Some(&gtk4::Label::new(Some(""))));
		});

		factory.connect_bind(|_, li| {
			let lh = li.downcast_ref::<gtk4::ListHeader>().expect("to be a ListHeader");
			let child = lh.child().and_downcast::<gtk4::Label>().unwrap();
			let item = lh.item().and_downcast::<CandidateObject>().unwrap();

			let provider = PROVIDERS.iter().find(|p| p.id().0 == item.provider().as_str()).unwrap();

			child.set_label(provider.name());
		});

		factory.connect_unbind(|_, li| {
			let lh = li.downcast_ref::<gtk4::ListHeader>().expect("to be a ListHeader");
			let child = lh.child().and_downcast::<gtk4::Label>().unwrap();
			child.set_label("");
		});

		factory
	}
}

#[derive(Default)]
struct LauncherContext {}

#[async_trait::async_trait]
impl ProviderContext for LauncherContext {
	async fn hide(&self) {}
	async fn close(&self) {}
	async fn set_input(&self, _input: String) {}
	async fn set_preview(&self, _preview: PreviewModel) {}
	async fn set_response(&self, _response: String) {}
}
