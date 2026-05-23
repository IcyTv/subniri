use iced::Length;
use iced::widget::{container, row, stack, text};
use iced::window::Id;
use iced::{Color, Element, Event, Subscription, Task, Theme, event};
use iced_layershell::actions::{IcedNewPopupSettings, PopupPlacement, PopupSize};
use iced_layershell::reexport::{Anchor, xdg_positioner::ConstraintAdjustment};
use iced_layershell::settings::{LayerShellSettings, StartMode};
use iced_layershell::{Settings, daemon, to_layer_message};
use wayland_client::Connection;

use crate::modules::{Module, ModuleKind, ModuleMessage};
use crate::style::COLORS;
use crate::widgets::neo_card;

mod animation;
mod modules;
mod style;
mod widgets;
#[macro_use]
mod icons;

// mod clock;
// mod icons;
// mod mpris;
fn main() -> Result<(), iced_layershell::Error> {
    let _ = pretty_env_logger::try_init();

    let connection = Connection::connect_to_env().unwrap();

    let app = daemon(
        {
            let conn = connection.clone();
            move || Bar::new(conn.clone())
        },
        Bar::namespace,
        Bar::update,
        Bar::view,
    )
    .style(Bar::style)
    .subscription(Bar::subscription)
    .settings(Settings {
        with_connection: Some(connection.into()),
        default_text_size: 18.into(),
        ..Default::default()
    })
    .layer_settings(LayerShellSettings {
        size: Some((0, 60)),
        exclusive_zone: 60,
        anchor: Anchor::Top | Anchor::Left | Anchor::Right,
        start_mode: StartMode::AllScreens,
        ..Default::default()
    });

    app.run()
}

#[allow(dead_code)]
fn scale_for_screen(height: u32) -> f32 {
    const BASE_SCREEN_HEIGHT: f32 = 1440.0;
    const SCREEN_SCALE_EXPONENT: f32 = 0.75;
    if height == 0 {
        return 1.0;
    }
    let linear_scale = height as f32 / BASE_SCREEN_HEIGHT;
    linear_scale.powf(SCREEN_SCALE_EXPONENT).clamp(0.7, 1.25)
}

struct Bar {
    left: Vec<Module>,
    center: Vec<Module>,
    right: Vec<Module>,

    open_popup: Option<(Id, Section, usize)>,
}

impl Bar {
    fn new(connection: Connection) -> (Self, Task<BarMessage>) {
        let _ = connection;

        let bar = Self {
            left: vec![Module::SystemMenu, Module::taskbar()],
            center: vec![Module::media_controls()],
            right: vec![
                Module::volume(),
                Module::Network,
                Module::bluetooth(),
                Module::clock(),
            ],
            open_popup: None,
        };

        let tasks = Task::batch([
            Self::module_init_tasks(Section::Left, &bar.left),
            Self::module_init_tasks(Section::Center, &bar.center),
            Self::module_init_tasks(Section::Right, &bar.right),
        ]);

        (bar, tasks)
    }

    fn namespace() -> String {
        String::from("polarbar-daemon")
    }

    fn update(&mut self, message: BarMessage) -> Task<BarMessage> {
        match message {
            BarMessage::WindowClosed(id) => {
                if self.open_popup.as_ref().map_or(false, |oid| oid.0 == id) {
                    self.open_popup = None;
                }
                Task::none()
            }
            BarMessage::Module(section, index, ModuleMessage::OpenPopup(kind, bounds)) => {
                let id = Id::unique();

                let task = if let Some(open_popup_id) = self.open_popup.take() {
                    iced_runtime::task::effect(iced_runtime::Action::Window(
                        iced_runtime::window::Action::Close(open_popup_id.0),
                    ))
                } else {
                    Task::none()
                };
                self.open_popup = Some((id, section, index));

                task.chain(Task::done(BarMessage::NewPopUp {
                    settings: IcedNewPopupSettings {
                        size: PopupSize::FitContent {
                            min: (1, 1),
                            max: (480, 640),
                        },
                        anchor_rect: (
                            bounds.x.round() as i32,
                            bounds.y.round() as i32,
                            bounds.width.round() as i32,
                            bounds.height.round() as i32,
                        ),
                        offset: (0, 8),
                        placement: PopupPlacement::BottomCenter,
                        constraint_adjustment: ConstraintAdjustment::SlideX
                            | ConstraintAdjustment::SlideY
                            | ConstraintAdjustment::FlipX
                            | ConstraintAdjustment::FlipY,
                    },
                    id,
                }))
                .chain(Task::done(BarMessage::SetPopupId(section, index, kind, id)))
            }
            BarMessage::SetPopupId(section, index, _kind, id) => {
                if let Some(module) = self.module_mut(section, index) {
                    module.set_popup_id(id);
                    Task::none()
                } else {
                    Task::done(BarMessage::RemoveWindow(id))
                }
            }
            BarMessage::Module(section, index, message) => {
                if let Some(module) = self.module_mut(section, index) {
                    return module
                        .update(message)
                        .map(move |msg| BarMessage::Module(section, index, msg));
                }

                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn view(&self, id: iced::window::Id) -> Element<'_, BarMessage> {
        if let Some((wid, section, index)) = &self.open_popup
            && *wid == id
        {
            // neo_card("A").background(COLORS.background).into()
            if let Some(module) = self.module(*section, *index) {
                module
                    .view_popup()
                    .map(move |message| BarMessage::Module(*section, *index, message))
            } else {
                neo_card(text("Something went wrong").color(COLORS.text))
                    .background(COLORS.feedback.danger90)
                    .into()
            }
        } else {
            let output_name = iced_layershell::window::output_name(id);
            let output_name = output_name.as_deref();

            stack![
                container(self.section(Section::Left, &self.left, output_name))
                    .align_left(Length::Fill)
                    .padding([4, 16]),
                container(self.section(Section::Center, &self.center, output_name))
                    .center_x(Length::Fill)
                    .padding([4, 16]),
                container(self.section(Section::Right, &self.right, output_name))
                    .align_right(Length::Fill)
                    .padding([4, 16]),
            ]
            .into()
        }
    }

    fn section<'a>(
        &self,
        section: Section,
        modules: &'a [Module],
        output_name: Option<&str>,
    ) -> Element<'a, BarMessage> {
        modules
            .iter()
            .enumerate()
            .fold(row![], |row, (index, module)| {
                row.push(
                    module
                        .view(output_name)
                        .map(move |message| BarMessage::Module(section, index, message)),
                )
            })
            .spacing(10.)
            .into()
    }

    fn module_mut(&mut self, section: Section, index: usize) -> Option<&mut Module> {
        match section {
            Section::Left => self.left.get_mut(index),
            Section::Center => self.center.get_mut(index),
            Section::Right => self.right.get_mut(index),
        }
    }

    fn module(&self, section: Section, index: usize) -> Option<&Module> {
        match section {
            Section::Left => self.left.get(index),
            Section::Center => self.center.get(index),
            Section::Right => self.right.get(index),
        }
    }

    fn style(&self, theme: &Theme) -> iced::theme::Style {
        iced::theme::Style {
            // background_color: Color::from_rgba(1.0, 0.0, 0.0, 0.5),
            background_color: Color::TRANSPARENT,
            text_color: theme.palette().background.base.text,
        }
    }

    fn subscription(&self) -> Subscription<BarMessage> {
        let mut subscriptions = vec![
            event::listen().map(BarMessage::IcedEvent),
            iced::window::close_events().map(BarMessage::WindowClosed),
        ];

        subscriptions.extend(Self::module_subscriptions(Section::Left, &self.left));
        subscriptions.extend(Self::module_subscriptions(Section::Center, &self.center));
        subscriptions.extend(Self::module_subscriptions(Section::Right, &self.right));

        Subscription::batch(subscriptions)
    }

    fn module_subscriptions(
        section: Section,
        modules: &[Module],
    ) -> impl Iterator<Item = Subscription<BarMessage>> + '_ {
        modules.iter().enumerate().map(move |(index, module)| {
            module
                .subscription()
                .with((section, index))
                .map(|((section, index), message)| BarMessage::Module(section, index, message))
        })
    }

    fn module_init_tasks(section: Section, modules: &[Module]) -> Task<BarMessage> {
        Task::batch(modules.iter().enumerate().map(move |(index, module)| {
            module
                .init_task()
                .map(move |message| BarMessage::Module(section, index, message))
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Section {
    Left,
    Center,
    Right,
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
enum BarMessage {
    #[allow(dead_code)]
    IcedEvent(Event),
    WindowClosed(Id),
    Module(Section, usize, ModuleMessage),
    SetPopupId(Section, usize, ModuleKind, Id),
}
