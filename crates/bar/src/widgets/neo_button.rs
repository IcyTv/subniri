use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use iced::{
    Animation, Color, Element, Event, Length, Padding, Rectangle,
    advanced::{
        Layout, Widget, layout, mouse, renderer,
        widget::{self, Tree, tree},
    },
    window,
};

use super::neo_surface::{self, NeoSurfaceStyle};

pub type NeoButtonStyle = NeoSurfaceStyle;

pub fn neo_button<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> NeoButton<'a, Message, Theme, Renderer> {
    NeoButton::new(content)
}

pub struct NeoButton<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_press: Option<OnPress<'a, Message>>,
    style: NeoButtonStyle,
    width: Length,
    height: Length,
    enabled: bool,
}

enum OnPress<'a, Message> {
    Message(Message),
    WithBounds(Rc<dyn Fn(Rectangle) -> Message + 'a>),
}

impl<Message: Clone> Clone for OnPress<'_, Message> {
    fn clone(&self) -> Self {
        match self {
            Self::Message(message) => Self::Message(message.clone()),
            Self::WithBounds(callback) => Self::WithBounds(callback.clone()),
        }
    }
}

impl<'a, Message, Theme, Renderer> NeoButton<'a, Message, Theme, Renderer> {
    pub fn new(content: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            content: content.into(),
            on_press: None,
            style: NeoButtonStyle::default(),
            width: Length::Shrink,
            height: Length::Shrink,
            enabled: true,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_press(mut self, message: Message) -> Self {
        self.on_press = Some(OnPress::Message(message));
        self
    }

    pub fn on_press_with_bounds<F>(mut self, callback: F) -> Self
    where
        F: Fn(Rectangle) -> Message + 'a,
    {
        self.on_press = Some(OnPress::WithBounds(Rc::new(callback)));
        self
    }

    pub fn style(mut self, style: NeoButtonStyle) -> Self {
        self.style = style;
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    pub fn background(mut self, color: Color) -> Self {
        self.style.background = color;
        self
    }

    pub fn disabled_background(mut self, color: Color) -> Self {
        self.style.disabled_background = color;
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.style.radius = radius;
        self
    }

    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.style.padding = padding.into();
        self
    }

    pub fn shadow_width(mut self, shadow_width: f32) -> Self {
        self.style.shadow_width = shadow_width;
        self
    }

    pub fn map<F, Out>(self, func: F) -> NeoButton<'a, Out, Theme, Renderer>
    where
        F: Fn(Message) -> Out + Clone + 'a,
        Renderer: iced::advanced::Renderer + 'a,
        Message: 'a,
        Theme: 'a,
        Out: 'a,
    {
        NeoButton {
            content: self.content.map(func.clone()),
            on_press: self.on_press.map(|on_press| match on_press {
                OnPress::Message(message) => OnPress::Message(func(message)),
                OnPress::WithBounds(callback) => {
                    OnPress::WithBounds(Rc::new(move |bounds| func(callback(bounds))))
                }
            }),
            style: self.style,
            width: self.width,
            height: self.height,
            enabled: self.enabled,
        }
    }
}

impl<'a, Message, Theme: 'a, Renderer: iced::advanced::Renderer + 'a>
    From<NeoButton<'a, Message, Theme, Renderer>> for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
{
    fn from(button: NeoButton<'a, Message, Theme, Renderer>) -> Self {
        Self::new(button)
    }
}

impl<'a, Message, Theme: 'a, Renderer: 'a> Widget<Message, Theme, Renderer>
    for NeoButton<'a, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: renderer::Renderer,
{
    fn size(&self) -> iced::Size<Length> {
        neo_surface::size(self.width, self.height)
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(State::default())
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        neo_surface::layout(
            &mut self.content,
            tree,
            renderer,
            limits,
            self.width,
            self.height,
            self.style,
        )
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if !self.enabled {
            return;
        }

        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let over = cursor.is_over(bounds);

        state.hovered = over;

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => {
                state.pressed = state.pressed.clone().go(true, Instant::now());

                shell.request_redraw();
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.pressed.value() =>
            {
                if over {
                    if let Some(message) = self.on_press.clone() {
                        shell.publish(match message {
                            OnPress::Message(message) => message,
                            OnPress::WithBounds(callback) => callback(bounds),
                        });
                    }
                }
                // state.pressed = false;
                state.pressed = state.pressed.clone().go(false, Instant::now());
                shell.request_redraw();
                shell.capture_event();
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                if state.pressed.is_animating(*now) {
                    shell.request_redraw();
                }
            }
            _ => (),
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if self.enabled && self.on_press.is_some() && cursor.is_over(layout.bounds()) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();

        let offset = state
            .pressed
            .interpolate(0.0, self.style.shadow_width, Instant::now());

        let child_layout = layout.child(0);

        neo_surface::draw(
            renderer,
            bounds,
            NeoSurfaceStyle {
                background: if self.enabled {
                    self.style.background
                } else {
                    self.style.disabled_background
                },
                ..self.style
            },
            offset,
            |renderer| {
                self.content.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    child_layout,
                    cursor,
                    viewport,
                );
            },
        );
    }
}

#[derive(Debug)]
struct State {
    pressed: Animation<bool>,
    hovered: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            pressed: Animation::new(false)
                .delay(Duration::ZERO)
                .duration(Duration::from_millis(50)),
            hovered: false,
        }
    }
}
