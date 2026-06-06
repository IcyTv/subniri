pub use kdl;

use miette::{Diagnostic, NamedSource, SourceSpan};
use std::sync::Arc;

type ConfigSource = Arc<NamedSource<String>>;

#[derive(thiserror::Error, Diagnostic, Debug)]
pub enum ConfigError {
	#[error("Missing field {name}")]
	#[diagnostic(code(config::missing_field))]
	MissingField {
		name: String,
		#[label]
		span: Option<SourceSpan>,
		#[source_code]
		src: Option<ConfigSource>,
	},
	#[error("Wrong type")]
	#[diagnostic(code(config::wrong_type))]
	WrongType {
		#[label]
		span: Option<SourceSpan>,
		#[source_code]
		src: Option<ConfigSource>,
	},
	#[error("Wrong type, expected: {expected}")]
	#[diagnostic(code(config::wrong_type))]
	WrongTypeExpected {
		#[label]
		span: Option<SourceSpan>,
		#[source_code]
		src: Option<ConfigSource>,
		expected: String,
	},
	#[error("Unknown field {name}")]
	#[diagnostic(code(config::unknown_field))]
	UnknownField {
		name: String,
		#[label]
		span: Option<SourceSpan>,
		#[source_code]
		src: Option<ConfigSource>,
	},
	#[error("Duplicate field {name}")]
	#[diagnostic(code(config::duplicate_field))]
	DuplicateField {
		name: String,
		#[label]
		span: Option<SourceSpan>,
		#[source_code]
		src: Option<ConfigSource>,
	},
	#[error("Out of range")]
	#[diagnostic(code(config::out_of_range))]
	OutOfRange {
		#[label]
		span: Option<SourceSpan>,
		#[source_code]
		src: Option<ConfigSource>,
	},
	#[error(transparent)]
	#[diagnostic(transparent)]
	ParseError(#[from] kdl::KdlError),
	#[error("IO error: {0}")]
	IoError(#[from] std::io::Error),
	#[error("Invalid date/time")]
	#[diagnostic(code(config::invalid_time))]
	InvalidDateTime {
		#[source]
		error: jiff::Error,
		#[source_code]
		src: Option<ConfigSource>,
		#[label]
		span: Option<SourceSpan>,
	},
	#[error("Error parsing date and time")]
	#[diagnostic(code(config::invalid_time))]
	DateTimeParse {
		#[source]
		error: anyhow::Error,
		#[source_code]
		src: Option<ConfigSource>,
		#[label]
		span: Option<SourceSpan>,
	},
	#[error("Invalid url")]
	#[diagnostic(code(config::invalid_url))]
	InvalidUrl {
		#[source]
		error: url::ParseError,
		#[source_code]
		src: Option<ConfigSource>,
		#[label]
		span: Option<SourceSpan>,
	},
	#[error("Config validation failed: {error}")]
	#[diagnostic(code(config::validation_failed))]
	Validation {
		#[source]
		error: Box<garde::error::Report>,
		#[source_code]
		src: Option<ConfigSource>,
		#[label]
		span: Option<SourceSpan>,
	},
}

impl std::ops::AddAssign for ConfigError {
	fn add_assign(&mut self, rhs: Self) {
		*self = rhs;
	}
}

impl From<url::ParseError> for ConfigError {
	fn from(value: url::ParseError) -> Self {
		Self::InvalidUrl {
			error: value,
			src: None,
			span: None,
		}
	}
}

impl ConfigError {
	pub fn missing_field(name: impl Into<String>, span: Option<SourceSpan>) -> Self {
		Self::MissingField {
			name: name.into(),
			span,
			src: None,
		}
	}

	pub fn wrong_type(span: Option<SourceSpan>) -> Self {
		Self::WrongType { span, src: None }
	}

	pub fn unknown_field(name: impl Into<String>, span: Option<SourceSpan>) -> Self {
		Self::UnknownField {
			name: name.into(),
			span,
			src: None,
		}
	}

	pub fn duplicate_field(name: impl Into<String>, span: Option<SourceSpan>) -> Self {
		Self::DuplicateField {
			name: name.into(),
			span,
			src: None,
		}
	}

	pub fn out_of_range(span: Option<SourceSpan>) -> Self {
		Self::OutOfRange { span, src: None }
	}

	pub fn invalid_date_time(e: jiff::Error, span: Option<SourceSpan>) -> Self {
		Self::InvalidDateTime {
			error: e,
			src: None,
			span,
		}
	}

	pub fn date_time_parse(e: anyhow::Error, span: Option<SourceSpan>) -> Self {
		Self::DateTimeParse {
			error: e,
			src: None,
			span,
		}
	}

	pub fn validation(error: garde::error::Report, span: Option<SourceSpan>) -> Self {
		Self::Validation {
			src: None,
			span,
			error: Box::new(error),
		}
	}

	pub fn expected(self, ty: impl Into<String>) -> Self {
		let Self::WrongType { span, src } = self else {
			#[cfg(debug_assertions)]
			panic!("Tried to add expected field to non-wrong type error");
			#[cfg(not(debug_assertions))]
			return self;
		};

		Self::WrongTypeExpected {
			span,
			src,
			expected: ty.into(),
		}
	}

	pub fn with_span_no_overwrite(self, new_span: SourceSpan) -> Self {
		match self {
			Self::MissingField { name, span, src } if span.is_none() => Self::MissingField {
				name,
				span: Some(new_span),
				src,
			},
			Self::WrongType { span, src } if span.is_none() => Self::WrongType {
				span: Some(new_span),
				src,
			},
			Self::WrongTypeExpected {
				span,
				src,
				expected,
			} if span.is_none() => Self::WrongTypeExpected {
				span: Some(new_span),
				src,
				expected,
			},
			Self::UnknownField { name, span, src } if span.is_none() => Self::UnknownField {
				name,
				span: Some(new_span),
				src,
			},
			Self::DuplicateField { name, span, src } if span.is_none() => Self::DuplicateField {
				name,
				span: Some(new_span),
				src,
			},
			Self::OutOfRange { span, src } if span.is_none() => Self::OutOfRange {
				span: Some(new_span),
				src,
			},
			Self::InvalidDateTime { error, src, span } if span.is_none() => Self::InvalidDateTime {
				error,
				src,
				span: Some(new_span),
			},
			Self::DateTimeParse { error, src, span } if span.is_none() => Self::DateTimeParse {
				error,
				src,
				span: Some(new_span),
			},
			Self::InvalidUrl { error, src, span } if span.is_none() => Self::InvalidUrl {
				error,
				src,
				span: Some(new_span),
			},
			Self::Validation { error, src, span } if span.is_none() => Self::Validation {
				error,
				src,
				span: Some(new_span),
			},
			_ => self,
		}
	}

	pub fn with_source(self, source: impl Into<String>) -> Self {
		let source = source.into();
		let source = || Arc::new(NamedSource::new("config", source.clone()));
		match self {
			Self::MissingField { name, span, .. } => Self::MissingField {
				name,
				span,
				src: Some(source()),
			},
			Self::WrongType { span, .. } => Self::WrongType {
				span,
				src: Some(source()),
			},
			Self::WrongTypeExpected { span, expected, .. } => Self::WrongTypeExpected {
				span,
				src: Some(source()),
				expected,
			},
			Self::UnknownField { name, span, .. } => Self::UnknownField {
				name,
				span,
				src: Some(source()),
			},
			Self::DuplicateField { name, span, .. } => Self::DuplicateField {
				name,
				span,
				src: Some(source()),
			},
			Self::OutOfRange { span, .. } => Self::OutOfRange {
				span,
				src: Some(source()),
			},
			Self::InvalidDateTime { error, span, .. } => Self::InvalidDateTime {
				error,
				src: Some(source()),
				span,
			},
			Self::DateTimeParse { error, span, .. } => Self::DateTimeParse {
				error,
				src: Some(source()),
				span,
			},
			Self::InvalidUrl { error, span, .. } => Self::InvalidUrl {
				error,
				src: Some(source()),
				span,
			},
			Self::Validation { error, span, .. } => Self::Validation {
				error,
				src: Some(source()),
				span,
			},
			err @ Self::ParseError(_) => err,
			err @ Self::IoError(_) => err,
		}
	}
}

pub trait Config: Sized {
	// fn from_doc(doc: &kdl::KdlDocument) -> Result<Self, ConfigError>;

	fn from_kdl_node(node: &kdl::KdlNode) -> Result<Self, ConfigError>;
}

pub trait ConfigValidateExt: Config + garde::Validate
where
	<Self as garde::Validate>::Context: Default,
{
	fn from_kdl_node_validated(node: &kdl::KdlNode) -> Result<Self, ConfigError> {
		let config = Self::from_kdl_node(node)?;
		config
			.validate()
			.map_err(|err| ConfigError::validation(err, Some(node.span())))?;
		Ok(config)
	}
}

impl<T> ConfigValidateExt for T
where
	T: Config + garde::Validate,
	<T as garde::Validate>::Context: Default,
{
}

pub trait ConfigFile: Sized {
	fn from_kdl_document(doc: &kdl::KdlDocument) -> Result<Self, ConfigError>;

	fn parse(input: &str) -> Result<(kdl::KdlDocument, Self), ConfigError> {
		let doc = kdl::KdlDocument::parse(input)?;
		match Self::from_kdl_document(&doc) {
			Ok(config) => Ok((doc, config)),
			Err(err) => Err(err.with_source(input)),
		}
	}
}

pub trait ConfigFileValidateExt: ConfigFile + garde::Validate
where
	<Self as garde::Validate>::Context: Default,
{
	fn from_kdl_document_validated(doc: &kdl::KdlDocument) -> Result<Self, ConfigError> {
		let config = Self::from_kdl_document(doc)?;
		config
			.validate()
			.map_err(|err| ConfigError::validation(err, Some(doc.span())))?;
		Ok(config)
	}

	fn parse_validated(input: &str) -> Result<(kdl::KdlDocument, Self), ConfigError> {
		let doc = kdl::KdlDocument::parse(input)?;
		match Self::from_kdl_document_validated(&doc) {
			Ok(config) => Ok((doc, config)),
			Err(err) => Err(err.with_source(input)),
		}
	}
}

impl<T> ConfigFileValidateExt for T
where
	T: ConfigFile + garde::Validate,
	<T as garde::Validate>::Context: Default,
{
}

pub trait ConfigValue: Sized {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError>;
	fn to_kdl_value(&self) -> kdl::KdlValue;
}

pub trait ConfigSerialize: Sized {
	fn apply_to_kdl_node(&self, node: &mut kdl::KdlNode) -> Result<(), ConfigError>;
}

pub trait ConfigFileSerialize: Sized {
	fn apply_to_kdl_document(&self, doc: &mut kdl::KdlDocument) -> Result<(), ConfigError>;
}

impl<T: ConfigValue> Config for T {
	fn from_kdl_node(node: &kdl::KdlNode) -> Result<Self, ConfigError> {
		if node.children().is_some() {
			return Err(ConfigError::wrong_type(None).expected("no children"));
		}

		if node.entries().is_empty() {
			return Err(ConfigError::missing_field("value", None));
		}

		if node.entries().len() > 1 {
			return Err(ConfigError::wrong_type(None).expected("one argument"));
		}

		let entry = node.entries().first().unwrap();

		if entry.name().is_some() {
			return Err(ConfigError::wrong_type(Some(entry.span())).expected("argument"));
		}

		let value = T::from_kdl_value(entry.value())?;

		Ok(value)
	}
}

impl<T: ConfigSerialize> ConfigSerialize for Option<T> {
	fn apply_to_kdl_node(&self, node: &mut kdl::KdlNode) -> Result<(), ConfigError> {
		match self {
			Some(value) => value.apply_to_kdl_node(node),
			None => {
				node.entries_mut().clear();
				node.entries_mut()
					.push(kdl::KdlEntry::new(kdl::KdlValue::Null));
				node.clear_children();
				Ok(())
			}
		}
	}
}

impl<T: ConfigValue> ConfigSerialize for T {
	fn apply_to_kdl_node(&self, node: &mut kdl::KdlNode) -> Result<(), ConfigError> {
		node.entries_mut().clear();
		node.entries_mut()
			.push(kdl::KdlEntry::new(self.to_kdl_value()));
		node.clear_children();
		Ok(())
	}
}

impl ConfigValue for String {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		match value {
			kdl::KdlValue::String(s) => Ok(s.clone()),
			kdl::KdlValue::Bool(b) => Ok(b.to_string()),
			kdl::KdlValue::Float(f) => Ok(f.to_string()),
			kdl::KdlValue::Integer(i) => Ok(i.to_string()),
			_ => Err(ConfigError::wrong_type(None)),
		}
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		kdl::KdlValue::String(self.clone())
	}
}

impl ConfigValue for bool {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		match value {
			kdl::KdlValue::Bool(b) => Ok(*b),
			_ => Err(ConfigError::wrong_type(None)),
		}
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		kdl::KdlValue::Bool(*self)
	}
}

impl ConfigValue for f32 {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		match value {
			kdl::KdlValue::Float(f) => Ok(*f as Self),
			kdl::KdlValue::Integer(i) => Ok((*i) as Self),
			_ => Err(ConfigError::wrong_type(None)),
		}
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		kdl::KdlValue::Float((*self) as f64)
	}
}

impl ConfigValue for f64 {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		match value {
			kdl::KdlValue::Float(f) => Ok(*f),
			kdl::KdlValue::Integer(i) => Ok((*i) as Self),
			_ => Err(ConfigError::wrong_type(None)),
		}
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		kdl::KdlValue::Float(*self)
	}
}

macro_rules! config_value_for_kdl_ty {
    ($kdl_ty:ident; $ty:ty) => {
        impl ConfigValue for $ty {
            fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
                match value {
                    kdl::KdlValue::$kdl_ty(int) => {
                        (*int).try_into().map_err(|_| ConfigError::out_of_range(None))
                    }
                    _ => Err(ConfigError::wrong_type(None)),
                }
            }

            fn to_kdl_value(&self) -> kdl::KdlValue {
                kdl::KdlValue::$kdl_ty(i128::try_from(*self).unwrap_or(i128::MAX))
            }
        }
    };

    ($kdl_ty:ident; $($ty:ty),*) => {
        $(
            config_value_for_kdl_ty!($kdl_ty; $ty);
        )*
    }
}

config_value_for_kdl_ty!(Integer; i8, i16, i32, i64, i128, u8, u16, u32, u64, u128);

impl ConfigValue for url::Url {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		match value {
			kdl::KdlValue::String(url) => url::Url::parse(url).map_err(Into::into),
			_ => Err(ConfigError::wrong_type(None)),
		}
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		kdl::KdlValue::String(self.to_string())
	}
}

impl ConfigValue for jiff::Timestamp {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		let string = match value {
			kdl::KdlValue::String(s) => s,
			_ => return Err(ConfigError::wrong_type(None)),
		};

		dateparser::parse(string)
			.map_err(|e| ConfigError::date_time_parse(e, None))
			.map(|dt| {
				jiff::Timestamp::new(dt.timestamp(), dt.timestamp_subsec_nanos() as i32).unwrap()
			})
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		// TODO: Better serialization?
		kdl::KdlValue::String(self.to_string())
	}
}

impl ConfigValue for jiff::civil::Time {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		let string = match value {
			kdl::KdlValue::String(s) => s,
			_ => return Err(ConfigError::wrong_type(None)),
		};

		string
			.parse()
			.map_err(|e| ConfigError::invalid_date_time(e, None))
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		let s = if self.second() == 0 {
			self.strftime("%H:%M").to_string()
		} else {
			format!("{:?}", self)
		};

		kdl::KdlValue::String(s)
	}
}

impl ConfigValue for jiff::SignedDuration {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		let string = match value {
			kdl::KdlValue::String(s) => s,
			_ => return Err(ConfigError::wrong_type(None)),
		};

		string
			.parse()
			.map_err(|e| ConfigError::invalid_date_time(e, None))
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		kdl::KdlValue::String(self.to_string())
	}
}

impl ConfigValue for jiff::Span {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		let string = match value {
			kdl::KdlValue::String(s) => s,
			_ => return Err(ConfigError::wrong_type(None)),
		};

		string
			.parse()
			.map_err(|e| ConfigError::invalid_date_time(e, None))
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		kdl::KdlValue::String(self.to_string())
	}
}

impl ConfigValue for std::path::PathBuf {
	fn from_kdl_value(value: &kdl::KdlValue) -> Result<Self, ConfigError> {
		match value {
			kdl::KdlValue::String(s) => Ok(std::path::PathBuf::from(s)),
			_ => Err(ConfigError::wrong_type(None)),
		}
	}

	fn to_kdl_value(&self) -> kdl::KdlValue {
		kdl::KdlValue::String(self.to_string_lossy().to_string())
	}
}
