//! Gif widget
//!
//! See <https://github.com/tarkah/iced_gif/blob/master/src/widget/gif.rs> for original
//! implementation.

use std::{
	fmt, io,
	time::{Duration, Instant},
};

use iced::{
	ContentFit, Element, Event, Length, Rectangle, Rotation, Size,
	advanced::{
		Layout, Shell, Widget,
		clipboard::Clipboard,
		layout, renderer,
		widget::{Tree, tree},
	},
	border,
	mouse::Cursor,
	widget::image::{FilterMethod, Handle},
	window,
};
use image::{AnimationDecoder, codecs::gif::GifDecoder};

pub fn gif<'a>(frames: &'a Frames) -> Gif<'a> {
	Gif::new(frames)
}

#[derive(Clone)]
pub struct Frames {
	frames: Vec<Frame>,
	total_bytes: usize,
}

impl Frames {
	pub fn from_bytes(bytes: &[u8]) -> Result<Self, image::ImageError> {
		let total_bytes = bytes.len();

		let cursor = io::Cursor::new(bytes);

		let decoder = GifDecoder::new(cursor)?;

		let frames = decoder
			.into_frames()
			.into_iter()
			.map(|result| result.map(Frame::from))
			.collect::<Result<Vec<_>, _>>()?;

		if frames.is_empty() {
			return Err(image::ImageError::Decoding(
				image::error::DecodingError::new(
					image::error::ImageFormatHint::Unknown,
					"GIF has no frames",
				),
			));
		}

		Ok(Self {
			frames,
			total_bytes,
		})
	}
}

impl fmt::Debug for Frames {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Frames")
			.field("frames", &self.frames.len())
			.field("total_bytes", &self.total_bytes)
			.finish()
	}
}

#[derive(Clone)]
struct Frame {
	delay: Duration,
	handle: Handle,
}

impl From<image::Frame> for Frame {
	fn from(frame: image::Frame) -> Self {
		let (width, height) = frame.buffer().dimensions();
		let delay = frame.delay().into();

		let handle = Handle::from_rgba(width, height, frame.into_buffer().into_vec());

		Self { delay, handle }
	}
}

pub struct Gif<'a> {
	frames: &'a Frames,

	width: Length,
	height: Length,
	crop: Option<Rectangle<u32>>,
	border_radius: border::Radius,
	content_fit: ContentFit,
	filter_method: FilterMethod,
	rotation: Rotation,
	opacity: f32,
	scale: f32,
	expand: bool,
}

impl<'a> Gif<'a> {
	/// Creates a new [`Gif`] with the given [`Frames`]
	pub fn new(frames: &'a Frames) -> Self {
		Gif {
			frames,
			width: Length::Shrink,
			height: Length::Shrink,
			crop: None,
			border_radius: border::Radius::default(),
			content_fit: ContentFit::default(),
			filter_method: FilterMethod::default(),
			rotation: Rotation::default(),
			opacity: 1.0,
			scale: 1.0,
			expand: false,
		}
	}
	/// Sets the width of the [`Image`] boundaries.
	pub fn width(mut self, width: impl Into<Length>) -> Self {
		self.width = width.into();
		self
	}

	/// Sets the height of the [`Image`] boundaries.
	pub fn height(mut self, height: impl Into<Length>) -> Self {
		self.height = height.into();
		self
	}

	/// Sets whether the [`Image`] should try to fill as much space
	/// available as possible while keeping aspect ratio and without
	/// allocating extra space in any axis with a [`Length::Shrink`]
	/// sizing strategy.
	///
	/// This is similar to using [`Length::Fill`] for both the
	/// [`width`](Self::width) and the [`height`](Self::height),
	/// but without the downside of blank space.
	pub fn expand(mut self, expand: bool) -> Self {
		self.expand = expand;
		self
	}

	/// Sets the [`ContentFit`] of the [`Image`].
	///
	/// Defaults to [`ContentFit::Contain`]
	pub fn content_fit(mut self, content_fit: ContentFit) -> Self {
		self.content_fit = content_fit;
		self
	}

	/// Sets the [`FilterMethod`] of the [`Image`].
	pub fn filter_method(mut self, filter_method: FilterMethod) -> Self {
		self.filter_method = filter_method;
		self
	}

	/// Applies the given [`Rotation`] to the [`Image`].
	pub fn rotation(mut self, rotation: impl Into<Rotation>) -> Self {
		self.rotation = rotation.into();
		self
	}

	/// Sets the opacity of the [`Image`].
	///
	/// It should be in the [0.0, 1.0] range—`0.0` meaning completely transparent,
	/// and `1.0` meaning completely opaque.
	pub fn opacity(mut self, opacity: impl Into<f32>) -> Self {
		self.opacity = opacity.into();
		self
	}

	/// Sets the scale of the [`Image`].
	///
	/// The region of the [`Image`] drawn will be scaled from the center by the given scale factor.
	/// This can be useful to create certain effects and animations, like smooth zoom in / out.
	pub fn scale(mut self, scale: impl Into<f32>) -> Self {
		self.scale = scale.into();
		self
	}

	/// Crops the [`Image`] to the given region described by the [`Rectangle`] in absolute
	/// coordinates.
	///
	/// Cropping is done before applying any transformation or [`ContentFit`]. In practice,
	/// this means that cropping an [`Image`] with this method should produce the same result
	/// as cropping it externally (e.g. with an image editor) and creating a new [`Handle`]
	/// for the cropped version.
	///
	/// However, this method is much more efficient; since it just leverages scissoring during
	/// rendering and no image cropping actually takes place. Instead, it reuses the existing
	/// image allocations and should be as efficient as not cropping at all!
	///
	/// The `region` coordinates will be clamped to the image dimensions, if necessary.
	pub fn crop(mut self, region: Rectangle<u32>) -> Self {
		self.crop = Some(region);
		self
	}

	/// Sets the [`border::Radius`] of the [`Image`].
	///
	/// Currently, it will only be applied around the rectangular bounding box
	/// of the [`Image`].
	pub fn border_radius(mut self, border_radius: impl Into<border::Radius>) -> Self {
		self.border_radius = border_radius.into();
		self
	}
}

struct State {
	index: usize,
	current: Current,
	total_bytes: usize,
}

impl State {
	fn new(frame: Frame, total_bytes: usize) -> Self {
		let current = Current {
			frame,
			started: Instant::now(),
		};

		Self {
			index: 0,
			current,
			total_bytes,
		}
	}
}

struct Current {
	frame: Frame,
	started: Instant,
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer> for Gif<'a>
where
	Renderer: iced::advanced::image::Renderer<Handle = Handle>,
{
	fn size(&self) -> Size<Length> {
		Size::new(self.width, self.height)
	}

	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<State>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(State::new(
			// This unwrap is safe because we check for empty frames in Frames::from_bytes
			self.frames.frames.first().cloned().unwrap(),
			self.frames.total_bytes,
		))
	}

	fn diff(&mut self, tree: &mut Tree) {
		let state = tree.state.downcast_mut::<State>();

		// Reset state if new gif Frames is used w/
		// same state tree.
		//
		// Total bytes of the gif should be a good enough
		// proxy for it changing.
		if state.total_bytes != self.frames.total_bytes {
			*state = State::new(
				// This unwrap is safe because we check for empty frames in Frames::from_bytes
				self.frames.frames.first().cloned().unwrap(),
				self.frames.total_bytes,
			);
		}
	}

	fn layout(
		&mut self, _tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits,
	) -> layout::Node {
		iced::widget::image::layout(
			renderer,
			limits,
			// This unwrap is safe because we check for empty frames in Frames::from_bytes
			&self.frames.frames.first().unwrap().handle,
			self.width,
			self.height,
			self.crop,
			self.content_fit,
			self.rotation,
			self.expand,
		)
	}

	fn update(
		&mut self, tree: &mut Tree, event: &Event, _layout: Layout<'_>, _cursor: Cursor,
		_renderer: &Renderer, shell: &mut Shell<'_, Message>, _viewport: &Rectangle,
	) {
		let state = tree.state.downcast_mut::<State>();

		if let Event::Window(window::Event::RedrawRequested(now)) = event {
			let elapsed = now.duration_since(state.current.started);

			if elapsed > state.current.frame.delay {
				state.index = (state.index + 1) % self.frames.frames.len();

				state.current = Current {
					frame: self.frames.frames[state.index].clone(),
					started: *now,
				};

				shell.request_redraw_at(*now + state.current.frame.delay);
			} else {
				let remaining = state.current.frame.delay - elapsed;

				shell.request_redraw_at(*now + remaining);
			}
		}
	}

	fn draw(
		&self, tree: &Tree, renderer: &mut Renderer, _theme: &Theme, _style: &renderer::Style,
		layout: Layout<'_>, _cursor: Cursor, _viewport: &Rectangle,
	) {
		let state = tree.state.downcast_ref::<State>();

		iced::widget::image::draw(
			renderer,
			layout,
			&state.current.frame.handle,
			self.crop,
			self.border_radius,
			self.content_fit,
			self.filter_method,
			self.rotation,
			self.opacity,
			self.scale,
		);
	}
}

impl<'a, Message, Theme, Renderer> From<Gif<'a>> for Element<'a, Message, Theme, Renderer>
where
	Renderer: iced::advanced::image::Renderer<Handle = Handle>,
{
	fn from(gif: Gif<'a>) -> Element<'a, Message, Theme, Renderer> {
		Element::new(gif)
	}
}
