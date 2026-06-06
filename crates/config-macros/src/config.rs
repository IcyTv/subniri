use convert_case::{Case, Casing};
use proc_macro_error2::abort;
use proc_macro2::TokenStream;
use quote::quote;
use syn::DeriveInput;

#[derive(deluxe::ExtractAttributes)]
#[deluxe(attributes(config))]
#[deluxe(crate = deluxe)]
struct ConfigOpts {
	#[deluxe(default)]
	children: Option<bool>,
	#[deluxe(default)]
	rename_all: RenameRule,
}

#[derive(deluxe::ExtractAttributes, Default)]
#[deluxe(attributes(config))]
#[deluxe(crate = deluxe)]
struct VariantAttrs {
	#[deluxe(default)]
	name: Option<String>,
}

#[derive(deluxe::ExtractAttributes, Default)]
#[deluxe(attributes(config))]
#[deluxe(crate = deluxe)]
struct FieldAttrs {
	#[deluxe(default)]
	parameter: bool,
	#[deluxe(default)]
	argument: bool,
	#[deluxe(default)]
	default: Option<DefaultAttr>,
	#[deluxe(default)]
	name: Option<String>,
	#[deluxe(default)]
	list_style: Option<ListStyle>,
	#[deluxe(default)]
	list_cutoff: Option<usize>,
	#[deluxe(default)]
	key: Option<String>,
}

#[derive(Debug)]
enum DefaultAttr {
	Flag,
	Expr(syn::Expr),
}

#[derive(Debug, Clone, Copy)]
enum ListStyle {
	Children,
	Inline,
	Auto,
}

#[derive(Debug, Default, Clone, Copy)]
enum RenameRule {
	Snake,
	#[default]
	Kebab,
	Camel,
	Pascal,
	ScreamingSnake,
	Lower,
	Upper,
}

impl RenameRule {
	fn case(self) -> Case<'static> {
		match self {
			Self::Snake => Case::Snake,
			Self::Kebab => Case::Kebab,
			Self::Camel => Case::Camel,
			Self::Pascal => Case::Pascal,
			Self::ScreamingSnake => Case::UpperSnake,
			Self::Lower => Case::Lower,
			Self::Upper => Case::Upper,
		}
	}
}

impl deluxe::ParseMetaItem for RenameRule {
	fn parse_meta_item(
		input: syn::parse::ParseStream, _mode: deluxe::ParseMode,
	) -> deluxe::Result<Self> {
		let (rule, span) = if input.peek(syn::LitStr) {
			let lit: syn::LitStr = input.parse()?;
			(lit.value(), lit.span())
		} else {
			let ident: syn::Ident = input.parse()?;
			(ident.to_string(), ident.span())
		};

		match rule.as_str() {
			"snake_case" => Ok(Self::Snake),
			"kebab-case" | "kebab_case" => Ok(Self::Kebab),
			"camelCase" | "camel_case" => Ok(Self::Camel),
			"PascalCase" | "pascal_case" => Ok(Self::Pascal),
			"SCREAMING_SNAKE_CASE" | "screaming_snake_case" => Ok(Self::ScreamingSnake),
			"lowercase" | "lower" => Ok(Self::Lower),
			"UPPERCASE" | "uppercase" | "upper" => Ok(Self::Upper),
			_ => Err(Into::into(syn::Error::new(
				span,
				"Expected a serde-style rename_all rule",
			))),
		}
	}
}

impl deluxe::ParseMetaItem for ListStyle {
	fn parse_meta_item(
		input: syn::parse::ParseStream, _mode: deluxe::ParseMode,
	) -> deluxe::Result<Self> {
		let (style, span) = if input.peek(syn::LitStr) {
			let lit: syn::LitStr = input.parse()?;
			(lit.value(), lit.span())
		} else {
			let ident: syn::Ident = input.parse()?;
			(ident.to_string(), ident.span())
		};

		match style.as_str() {
			"children" => Ok(Self::Children),
			"inline" => Ok(Self::Inline),
			"auto" => Ok(Self::Auto),
			_ => Err(Into::into(syn::Error::new(
				span,
				"Expected children, inline, or auto",
			))),
		}
	}
}

impl deluxe::ParseMetaItem for DefaultAttr {
	fn parse_meta_item(
		input: syn::parse::ParseStream, _mode: deluxe::ParseMode,
	) -> deluxe::Result<Self> {
		let expr = input.parse()?;
		Ok(Self::Expr(expr))
	}

	fn parse_meta_item_flag(_span: proc_macro2::Span) -> deluxe::Result<Self> {
		Ok(Self::Flag)
	}
}

pub fn generate_config(mut input: DeriveInput) -> Result<TokenStream, deluxe::Error> {
	let opts: ConfigOpts = deluxe::extract_attributes(&mut input)?;
	let name = &input.ident;
	let syn::Data::Struct(s) = &mut input.data else {
		return Ok(match &mut input.data {
			syn::Data::Enum(e) => generate_for_enum(&opts, name, &input.generics, e),
			syn::Data::Union(_) => abort!(input, "Unions are unsupported"),
			syn::Data::Struct(_) => unreachable!(),
		});
	};
	let initializer = generate_for_struct(&opts, s);
	let generics = &input.generics;

	Ok(quote! {
		#[automatically_derived]
		impl ::config_traits::Config for #name<#generics> {
			// fn from_doc(doc: &::config_traits::kdl::KdlDocument) -> Result<Self, ::config_traits::ConfigError> {
			fn from_kdl_node(doc: &::config_traits::kdl::KdlNode) -> Result<Self, ::config_traits::ConfigError> {
				#initializer
			}
		}
	})
}

pub fn generate_config_file(mut input: DeriveInput) -> Result<TokenStream, deluxe::Error> {
	let opts: ConfigOpts = deluxe::extract_attributes(&mut input)?;
	let name = &input.ident;
	let initializer = match &mut input.data {
		syn::Data::Struct(s) => generate_for_document(&opts, s),
		syn::Data::Enum(_) => abort!(input, "Enums cannot derive ConfigFile"),
		syn::Data::Union(_) => abort!(input, "Unions are unsupported"),
	};
	let generics = &input.generics;

	Ok(quote! {
		#[automatically_derived]
		impl ::config_traits::ConfigFile for #name<#generics> {
			fn from_kdl_document(doc: &::config_traits::kdl::KdlDocument) -> Result<Self, ::config_traits::ConfigError> {
				#initializer
			}
		}
	})
}

pub fn generate_config_serialize(mut input: DeriveInput) -> Result<TokenStream, deluxe::Error> {
	let opts: ConfigOpts = deluxe::extract_attributes(&mut input)?;
	let name = &input.ident;
	let body = match &mut input.data {
		syn::Data::Struct(s) => generate_serialize_for_struct(&opts, s),
		syn::Data::Enum(_) => return Ok(TokenStream::new()),
		syn::Data::Union(_) => abort!(input, "Unions are unsupported"),
	};
	let generics = &input.generics;

	Ok(quote! {
		#[automatically_derived]
		impl ::config_traits::ConfigSerialize for #name<#generics> {
			fn apply_to_kdl_node(&self, node: &mut ::config_traits::kdl::KdlNode) -> Result<(), ::config_traits::ConfigError> {
				#body
			}
		}
	})
}

pub fn generate_config_file_serialize(
	mut input: DeriveInput,
) -> Result<TokenStream, deluxe::Error> {
	let opts: ConfigOpts = deluxe::extract_attributes(&mut input)?;
	let name = &input.ident;
	let body = match &mut input.data {
		syn::Data::Struct(s) => generate_serialize_for_document(&opts, s),
		syn::Data::Enum(_) => abort!(input, "TODO enums"),
		syn::Data::Union(_) => abort!(input, "Unions are unsupported"),
	};
	let generics = &input.generics;

	Ok(quote! {
		#[automatically_derived]
		impl ::config_traits::ConfigFileSerialize for #name<#generics> {
			fn apply_to_kdl_document(&self, doc: &mut ::config_traits::kdl::KdlDocument) -> Result<(), ::config_traits::ConfigError> {
				#body
			}
		}
	})
}

fn generate_for_enum(
	opts: &ConfigOpts, name: &syn::Ident, generics: &syn::Generics, e: &mut syn::DataEnum,
) -> TokenStream {
	let errors = deluxe::Errors::new();
	let rename_rule = opts.rename_all;
	let mut parse_arms = Vec::new();
	let mut serialize_arms = Vec::new();
	let mut expected = Vec::new();

	for variant in &mut e.variants {
		if !matches!(variant.fields, syn::Fields::Unit) {
			abort!(variant, "Only unit enum variants are supported");
		}

		let attrs: VariantAttrs = deluxe::extract_attributes_optional(&mut variant.attrs, &errors);
		let variant_ident = &variant.ident;
		let variant_name = attrs
			.name
			.unwrap_or_else(|| variant_ident.to_string().to_case(rename_rule.case()));

		expected.push(variant_name.clone());
		parse_arms.push(quote! { #variant_name => Ok(Self::#variant_ident), });
		serialize_arms.push(quote! { Self::#variant_ident => #variant_name, });
	}

	let expected = expected.join(", ");
	let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

	quote! {
		#[automatically_derived]
		impl #impl_generics ::config_traits::ConfigValue for #name #type_generics #where_clause {
			fn from_kdl_value(value: &::config_traits::kdl::KdlValue) -> Result<Self, ::config_traits::ConfigError> {
				match value {
					::config_traits::kdl::KdlValue::String(value) => match value.as_str() {
						#(#parse_arms)*
						_ => Err(::config_traits::ConfigError::wrong_type(None).expected(#expected)),
					},
					_ => Err(::config_traits::ConfigError::wrong_type(None).expected(#expected)),
				}
			}

			fn to_kdl_value(&self) -> ::config_traits::kdl::KdlValue {
				::config_traits::kdl::KdlValue::String(match self {
					#(#serialize_arms)*
				}.to_string())
			}
		}
	}
}

fn generate_for_struct(_opts: &ConfigOpts, s: &mut syn::DataStruct) -> TokenStream {
	let mut initializer = Vec::new();
	let mut child_match = Vec::new();
	let mut seen_flags = Vec::new();
	let mut required_children = Vec::new();

	let errors = deluxe::Errors::new();

	for field in &mut s.fields {
		let attrs: FieldAttrs = deluxe::extract_attributes_optional(&mut field.attrs, &errors);

		let field_ident = match field.ident.as_ref() {
			Some(field) => field,
			None => abort!(field, "Field without a name"),
		};
		let field_name = attrs.name.unwrap_or_else(|| field_ident.to_string());

		let is_option = is_option(&field.ty);
		let has_default_attr = attrs.default.is_some();
		let default_expr = match attrs.default.as_ref() {
			Some(DefaultAttr::Expr(expr)) => Some(expr),
			_ => None,
		};
		let allow_missing = has_default_attr || is_option || default_expr.is_some();

		if attrs.parameter {
			if attrs.argument {
				abort!(field, "Field cannot be both parameter and argument");
			}
			let parse = parse_field(&field.ty);
			let fallback = default_fallback(default_expr, &field.ty, is_option, has_default_attr);
			initializer.push(quote! {
                {
                    if let Some(value) = doc.get(#field_name) {
                        out.#field_ident = #parse;
                    } else if #allow_missing {
                        out.#field_ident = #fallback;
                    } else {
                        return Err(::config_traits::ConfigError::missing_field(#field_name.to_string(), Some(doc.span())));
                    }
                }
            });
		} else if attrs.argument {
			let parse = parse_field(&field.ty);
			if attrs.argument {
   				let fallback =
   					default_fallback(default_expr, &field.ty, is_option, has_default_attr);
   				initializer.push(quote! {
                       {
                           if let Some(value) = doc.get(current_property) {
                               out.#field_ident = #parse;
                               current_property += 1;
                           } else if #allow_missing {
                               out.#field_ident = #fallback;
                           } else {
                               return Err(::config_traits::ConfigError::missing_field(#field_name.to_string(), Some(doc.span())));
                           }
                       }
                   });
   			} else {
   				let fallback =
   					default_fallback(default_expr, &field.ty, is_option, has_default_attr);
   				initializer.push(quote! {
                       {
                           if let Some(value) = doc.get(current_property) {
                               out.#field_ident = #parse;
                               current_property += 1;
                           } else if #allow_missing {
                               out.#field_ident = #fallback;
                           } else {
                               return Err(::config_traits::ConfigError::missing_field(#field_name.to_string(), Some(doc.span())));
                           }
                       }
                   });
   			}
		} else {
			let seen_ident = seen_ident(field_ident, &field.ty);
			if let Some(seen_ident) = seen_ident.as_ref() {
				let required = is_required_child(&field.ty, allow_missing);
				let fallback =
					default_fallback(default_expr, &field.ty, is_option, has_default_attr);
				seen_flags.push(quote! { let mut #seen_ident = false; });
				required_children.push(quote! {
                    if !#seen_ident {
                        if #allow_missing {
                            out.#field_ident = #fallback;
                        } else if #required {
                            return Err(::config_traits::ConfigError::missing_field(#field_name.to_string(), Some(doc.span())));
                        }
                    }
                });
			}
			if let Some(parse_child) = parse_child_field(
				&field.ty,
				&field_name,
				field_ident,
				seen_ident.as_ref(),
				default_expr,
				is_option,
				attrs.key.as_deref(),
			) {
				child_match.push(parse_child);
			}
		}
	}

	quote! {
		// let mut entries = doc.entries().into_iter();
		let mut out = Self::default();
		let mut current_property = 0;

		#(#initializer)*

		#(#seen_flags)*

		if let Some(children) = doc.children() {
			for child in children.nodes() {
				match child.name().value() {
					#(#child_match)*
					n => return Err(::config_traits::ConfigError::unknown_field(n.to_string(), Some(child.span())))
				}
			}
		}

		#(#required_children)*

		Ok(out)
	}
}

fn generate_for_document(opts: &ConfigOpts, s: &mut syn::DataStruct) -> TokenStream {
	let mut initializer = Vec::new();
	let mut node_match = Vec::new();
	let mut required_nodes = Vec::new();

	let errors = deluxe::Errors::new();

	if !opts.children.unwrap_or(true) {
		abort!(
			s.struct_token,
			"ConfigFile derives must use children-based parsing"
		);
	}

	for field in &mut s.fields {
		let attrs: FieldAttrs = deluxe::extract_attributes_optional(&mut field.attrs, &errors);
		if attrs.argument || attrs.parameter {
			abort!(field, "ConfigFile fields cannot be parameters or arguments");
		}

		let field_ident = match field.ident.as_ref() {
			Some(field) => field,
			None => abort!(field, "Field without a name"),
		};
		let field_name = attrs.name.unwrap_or_else(|| field_ident.to_string());

		let is_option = is_option(&field.ty);
		let has_default_attr = attrs.default.is_some();
		let default_expr = match attrs.default.as_ref() {
			Some(DefaultAttr::Expr(expr)) => Some(expr),
			_ => None,
		};
		let allow_missing = has_default_attr || is_option || default_expr.is_some();

		let seen_ident = seen_ident(field_ident, &field.ty);
		if let Some(seen_ident) = seen_ident.as_ref() {
			let required = is_required_child(&field.ty, allow_missing);
			let fallback = default_fallback(default_expr, &field.ty, is_option, has_default_attr);
			initializer.push(quote! { let mut #seen_ident = false; });
			required_nodes.push(quote! {
                if !#seen_ident {
                    if #allow_missing {
                        out.#field_ident = #fallback;
                    } else if #required {
                        return Err(::config_traits::ConfigError::missing_field(#field_name.to_string(), Some(doc.span())));
                    }
                }
            });
		}

		if let Some(parse_node) = parse_document_field(
			&field.ty,
			&field_name,
			field_ident,
			seen_ident.as_ref(),
			attrs.key.as_deref(),
		) {
			node_match.push(parse_node);
		}
	}

	quote! {
		let mut out = Self::default();

		#(#initializer)*

		for node in doc.nodes() {
			match node.name().value() {
				#(#node_match)*
				n => return Err(::config_traits::ConfigError::unknown_field(n.to_string(), Some(node.span())))
			}
		}

		#(#required_nodes)*

		Ok(out)
	}
}

fn generate_serialize_for_struct(_opts: &ConfigOpts, s: &mut syn::DataStruct) -> TokenStream {
	let mut initializer = Vec::new();
	let errors = deluxe::Errors::new();

	for field in &mut s.fields {
		let attrs: FieldAttrs = deluxe::extract_attributes_optional(&mut field.attrs, &errors);

		let field_ident = match field.ident.as_ref() {
			Some(field) => field,
			None => abort!(field, "Field without a name"),
		};
		let field_name = attrs.name.unwrap_or_else(|| field_ident.to_string());

		if attrs.parameter {
			if attrs.argument {
				abort!(field, "Field cannot be both parameter and argument");
			}
			let value_tokens = parameter_value_tokens(&field.ty, field_ident);
			initializer.push(quote! {
				{
					let value = #value_tokens;
					if let Some(entry) = node.entry_mut(#field_name) {
						*entry.value_mut() = value;
					} else {
						node.insert(
							#field_name,
							::config_traits::kdl::KdlEntry::new_prop(#field_name, value),
						);
					}
				}
			});
		} else if attrs.argument {
			let value_tokens = parameter_value_tokens(&field.ty, field_ident);
			initializer.push(quote! {
				{
					let value = #value_tokens;
					let entry = ::config_traits::kdl::KdlEntry::new(value);
					if node.entries().len() <= current_property {
						node.entries_mut().push(entry);
					} else {
						node.entries_mut()[current_property] = entry;
					}
					current_property += 1;
				}
			});
		} else {
			let docs = doc_comment(&field.attrs);
			if let Some(tokens) = serialize_child_update(
				&field.ty,
				&field_name,
				field_ident,
				attrs.list_style,
				attrs.list_cutoff,
				attrs.key.as_deref(),
				docs.as_deref(),
			) {
				initializer.push(tokens);
			}
		}
	}

	quote! {
		let mut current_property = 0;

		#(#initializer)*

		Ok(())
	}
}

fn generate_serialize_for_document(opts: &ConfigOpts, s: &mut syn::DataStruct) -> TokenStream {
	let mut initializer = Vec::new();
	let errors = deluxe::Errors::new();

	if !opts.children.unwrap_or(true) {
		abort!(
			s.struct_token,
			"ConfigFile derives must use children-based parsing"
		);
	}

	for field in &mut s.fields {
		let attrs: FieldAttrs = deluxe::extract_attributes_optional(&mut field.attrs, &errors);
		if attrs.argument || attrs.parameter {
			abort!(field, "ConfigFile fields cannot be parameters or arguments");
		}

		let field_ident = match field.ident.as_ref() {
			Some(field) => field,
			None => abort!(field, "Field without a name"),
		};
		let field_name = attrs.name.unwrap_or_else(|| field_ident.to_string());

		let tokens = serialize_document_update(
			&field.ty,
			&field_name,
			field_ident,
			attrs.list_style,
			attrs.list_cutoff,
			attrs.key.as_deref(),
			doc_comment(&field.attrs).as_deref(),
		);
		initializer.push(tokens);
	}

	quote! {
		#(#initializer)*

		Ok(())
	}
}

const KDL_VALUE_TYPES: &[&str] = &[
	"String", "f32", "f64", "bool", "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64",
	"u128",
];

fn parse_field(ty: &syn::Type) -> TokenStream {
	use syn::Type::Path;
	match ty {
		Path(_) if option_inner_ty(ty).is_none() => {
			quote! { <#ty as ::config_traits::ConfigValue>::from_kdl_value(value)? }
		}
		Path(p) if let Some(inner) = option_inner_ty(ty) => {
			quote! {
				match value {
					::config_traits::kdl::KdlValue::Null => None,
					_ => Some(<#inner as ::config_traits::ConfigValue>::from_kdl_value(value)?),
				}
			}
		}
		_ => abort!(ty, "Unsupported type"),
	}
}

fn parse_child_field(
	ty: &syn::Type, field_name: &str, field_ident: &syn::Ident, seen_ident: Option<&syn::Ident>,
	_default_expr: Option<&syn::Expr>, _is_option: bool, key: Option<&str>,
) -> Option<TokenStream> {
	if is_option(ty) {
		let inner = option_inner_ty(ty).unwrap();
		let parse = parse_child_value_or_config(inner, field_name);
		let seen = seen_ident.map(|ident| {
            quote! {
                if #ident {
                    return Err(::config_traits::ConfigError::duplicate_field(#field_name.to_string(), Some(child.span())));
                }
                #ident = true;
            }
        });
		return Some(quote! {
			#field_name => {
				if child.entries().len() == 1
					&& child.entries()[0].name().is_none()
					&& matches!(child.entries()[0].value(), ::config_traits::kdl::KdlValue::Null)
					&& child.children().is_none()
				{
					#seen
					out.#field_ident = None;
				} else {
					#parse
					#seen
					out.#field_ident = Some(value);
				}
			}
		});
	}

	if is_bool(ty) {
		let seen = seen_ident.map(|ident| {
            quote! {
                if #ident {
                    return Err(::config_traits::ConfigError::duplicate_field(#field_name.to_string(), Some(child.span())));
                }
                #ident = true;
            }
        });
		return Some(quote! {
			#field_name => {
				let entries = child.entries();
				let value = if entries.is_empty() {
					true
				} else if entries.len() == 1 {
					let entry = &entries[0];
					if entry.name().is_some() {
						return Err(::config_traits::ConfigError::wrong_type(Some(child.span())));
					}
					match entry.value() {
						::config_traits::kdl::KdlValue::Bool(value) => *value,
						_ => return Err(::config_traits::ConfigError::wrong_type(Some(child.span()))),
					}
				} else {
					return Err(::config_traits::ConfigError::wrong_type(Some(child.span())));
				};
				if child.children().is_some() {
					return Err(::config_traits::ConfigError::wrong_type(Some(child.span())));
				}
				#seen
				out.#field_ident = value;
			}
		});
	}

	if let Some(inner) = vec_inner_ty(ty) {
		if key.is_none() {
			return Some(quote! {
				#field_name => {
					let entries = child.entries();
					for entry in entries {
						if entry.name().is_some() {
							return Err(::config_traits::ConfigError::wrong_type(Some(child.span())));
						}
						let value = <#inner as ::config_traits::ConfigValue>::from_kdl_value(entry.value())
							.map_err(|e| e.with_span_no_overwrite(entry.span()))?;
						out.#field_ident.push(value);
					}

					if let Some(children) = child.children() {
						for dash in children.nodes() {
							if dash.name().value() == "-" {
								let entries = dash.entries();
								if entries.len() != 1 {
									return Err(::config_traits::ConfigError::wrong_type(Some(dash.span())));
								}
								if entries[0].name().is_some() || dash.children().is_some() {
									return Err(::config_traits::ConfigError::wrong_type(Some(dash.span())));
								}
								let value = <#inner as ::config_traits::ConfigValue>::from_kdl_value(entries[0].value())
									.map_err(|e| e.with_span_no_overwrite(entries[0].span()))?;
								out.#field_ident.push(value);
							} else {
								return Err(::config_traits::ConfigError::unknown_field(
									dash.name().value().to_string(),
									Some(dash.span()),
								));
							}
						}
					}
				}
			});
		}

		let parse = parse_child_value_or_config(inner, field_name);
		return Some(quote! {
			#field_name => {
				#parse
				out.#field_ident.push(value);
			}
		});
	}

	let parse = parse_child_value_or_config(ty, field_name);
	let seen = seen_ident.map(|ident| {
        quote! {
            if #ident {
                return Err(::config_traits::ConfigError::duplicate_field(#field_name.to_string(), Some(child.span())));
            }
            #ident = true;
        }
    });
	Some(quote! {
		#field_name => {
			#parse
			#seen
			out.#field_ident = value;
		}
	})
}

fn parse_child_value_or_config(ty: &syn::Type, field_name: &str) -> TokenStream {
	if is_value_type(ty) {
		quote! {
			let value = child.get(0).ok_or_else(|| ::config_traits::ConfigError::missing_field(#field_name.to_string(), Some(child.span())))?;
			let value = <#ty as ::config_traits::ConfigValue>::from_kdl_value(value)?;
		}
	} else {
		quote! {
			let value = <#ty as ::config_traits::Config>::from_kdl_node(child)
				.map_err(|e| e.with_span_no_overwrite(child.span()))?;
		}
	}
}

fn parse_document_field(
	ty: &syn::Type, field_name: &str, field_ident: &syn::Ident, seen_ident: Option<&syn::Ident>,
	key: Option<&str>,
) -> Option<TokenStream> {
	if is_option(ty) {
		let inner = option_inner_ty(ty).unwrap();
		let parse = parse_node_value_or_config(inner, field_name);
		let seen = seen_ident.map(|ident| {
            quote! {
                if #ident {
                    return Err(::config_traits::ConfigError::duplicate_field(#field_name.to_string(), Some(node.span())));
                }
                #ident = true;
            }
        });
		return Some(quote! {
			#field_name => {
				if node.entries().len() == 1
					&& node.entries()[0].name().is_none()
					&& matches!(node.entries()[0].value(), ::config_traits::kdl::KdlValue::Null)
					&& node.children().is_none()
				{
					#seen
					out.#field_ident = None;
				} else {
					#parse
					#seen
					out.#field_ident = Some(value);
				}
			}
		});
	}

	if is_bool(ty) {
		let seen = seen_ident.map(|ident| {
            quote! {
                if #ident {
                    return Err(::config_traits::ConfigError::duplicate_field(#field_name.to_string(), Some(node.span())));
                }
                #ident = true;
            }
        });
		return Some(quote! {
			#field_name => {
				let entries = node.entries();
				let value = if entries.is_empty() {
					true
				} else if entries.len() == 1 {
					let entry = &entries[0];
					if entry.name().is_some() {
						return Err(::config_traits::ConfigError::wrong_type(Some(node.span())));
					}
					match entry.value() {
						::config_traits::kdl::KdlValue::Bool(value) => *value,
						_ => return Err(::config_traits::ConfigError::wrong_type(Some(node.span()))),
					}
				} else {
					return Err(::config_traits::ConfigError::wrong_type(Some(node.span())));
				};
				if node.children().is_some() {
					return Err(::config_traits::ConfigError::wrong_type(Some(node.span())));
				}
				#seen
				out.#field_ident = value;
			}
		});
	}

	if let Some(inner) = vec_inner_ty(ty) {
		if key.is_none() {
			return Some(quote! {
				#field_name => {
					let entries = node.entries();
					for entry in entries {
						if entry.name().is_some() {
							return Err(::config_traits::ConfigError::wrong_type(Some(node.span())));
						}
						let value = <#inner as ::config_traits::ConfigValue>::from_kdl_value(entry.value())?;
						out.#field_ident.push(value);
					}

					if let Some(children) = node.children() {
						for dash in children.nodes() {
							if dash.name().value() == "-" {
								let entries = dash.entries();
								if entries.len() != 1 {
									return Err(::config_traits::ConfigError::wrong_type(Some(dash.span())));
								}
								if entries[0].name().is_some() || dash.children().is_some() {
									return Err(::config_traits::ConfigError::wrong_type(Some(dash.span())));
								}
								let value = <#inner as ::config_traits::ConfigValue>::from_kdl_value(entries[0].value())?;
								out.#field_ident.push(value);
							} else {
								return Err(::config_traits::ConfigError::unknown_field(
									dash.name().value().to_string(),
									Some(dash.span()),
								));
							}
						}
					}
				}
			});
		}

		let parse = parse_node_value_or_config(inner, field_name);
		return Some(quote! {
			#field_name => {
				#parse
				out.#field_ident.push(value);
			}
		});
	}

	let parse = parse_node_value_or_config(ty, field_name);
	let seen = seen_ident.map(|ident| {
        quote! {
            if #ident {
                return Err(::config_traits::ConfigError::duplicate_field(#field_name.to_string(), Some(node.span())));
            }
            #ident = true;
        }
    });
	Some(quote! {
		#field_name => {
			#parse
			#seen
			out.#field_ident = value;
		}
	})
}

fn list_style_bool(
	style: Option<ListStyle>, cutoff: Option<usize>, len_ident: &syn::Ident,
) -> TokenStream {
	match style.unwrap_or(ListStyle::Auto) {
		ListStyle::Children => quote! { true },
		ListStyle::Inline => quote! { false },
		ListStyle::Auto => {
			let cutoff = cutoff.unwrap_or(3);
			quote! { #len_ident > #cutoff }
		}
	}
}

fn parse_node_value_or_config(ty: &syn::Type, field_name: &str) -> TokenStream {
	if is_value_type(ty) {
		quote! {
			let value = node.get(0).ok_or_else(|| ::config_traits::ConfigError::missing_field(#field_name.to_string(), Some(node.span())))?;
			let value = <#ty as ::config_traits::ConfigValue>::from_kdl_value(value)?;
		}
	} else {
		quote! {
			let value = <#ty as ::config_traits::Config>::from_kdl_node(node)
				.map_err(|e| e.with_span_no_overwrite(node.span()))?;
		}
	}
}

fn parameter_value_tokens(ty: &syn::Type, field_ident: &syn::Ident) -> TokenStream {
	if is_option(ty) {
		let inner = option_inner_ty(ty).unwrap();
		if is_value_type(inner) {
			quote! {
				if let Some(value) = &self.#field_ident {
					<_ as ::config_traits::ConfigValue>::to_kdl_value(value)
				} else {
					::config_traits::kdl::KdlValue::Null
				}
			}
		} else {
			quote! {
				if let Some(value) = &self.#field_ident {
					let mut node = ::config_traits::kdl::KdlNode::new("_value");
					::config_traits::ConfigSerialize::apply_to_kdl_node(value, &mut node)?;
					let entry = node.entries().first().ok_or_else(|| ::config_traits::ConfigError::missing_field("value", Some(node.span())))?;
					entry.value().clone()
				} else {
					::config_traits::kdl::KdlValue::Null
				}
			}
		}
	} else {
		quote! { <_ as ::config_traits::ConfigValue>::to_kdl_value(&self.#field_ident) }
	}
}

fn serialize_child_update(
	ty: &syn::Type, field_name: &str, field_ident: &syn::Ident, list_style: Option<ListStyle>,
	list_cutoff: Option<usize>, key: Option<&str>, doc_comment: Option<&str>,
) -> Option<TokenStream> {
	let apply_doc_comment = apply_doc_comment(doc_comment, &quote! { new_node });
	if is_option(ty) {
		return Some(quote! {
			{
				let child_index = node.ensure_children().nodes().iter().position(|n| n.name().value() == #field_name);
				let child = match child_index {
					Some(index) => &mut node.ensure_children().nodes_mut()[index],
					None => {
						let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
						#apply_doc_comment
						node.ensure_children().nodes_mut().push(new_node);
						node.ensure_children().nodes_mut().last_mut().unwrap()
					}
				};
				::config_traits::ConfigSerialize::apply_to_kdl_node(&self.#field_ident, child)?;
			}
		});
	}

	if let Some(_inner) = vec_inner_ty(ty) {
		if key.is_none() {
			let len_ident = syn::Ident::new("__len", proc_macro2::Span::call_site());
			let use_children = list_style_bool(list_style, list_cutoff, &len_ident);
			return Some(quote! {
				{
					let child_index = node
						.ensure_children()
						.nodes()
						.iter()
						.position(|n| n.name().value() == #field_name);
					let child = match child_index {
						Some(index) => &mut node.ensure_children().nodes_mut()[index],
						None => {
							let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
							#apply_doc_comment
							node.ensure_children().nodes_mut().push(new_node);
							node.ensure_children().nodes_mut().last_mut().unwrap()
						}
					};

					let #len_ident = self.#field_ident.len();
					let use_children = #use_children;
					if use_children {
						child.entries_mut().clear();
						let dash_children = child.ensure_children();
						dash_children.nodes_mut().retain(|n| n.name().value() == "-");
						for (idx, value) in self.#field_ident.iter().enumerate() {
							let mut node = if idx < dash_children.nodes().len() {
								dash_children.nodes_mut()[idx].clone()
							} else {
								::config_traits::kdl::KdlNode::new("-")
							};
							node.entries_mut().clear();
							node.entries_mut().push(::config_traits::kdl::KdlEntry::new(
								<_ as ::config_traits::ConfigValue>::to_kdl_value(value),
							));
							node.clear_children();
							if idx < dash_children.nodes().len() {
								dash_children.nodes_mut()[idx] = node;
							} else {
								dash_children.nodes_mut().push(node);
							}
						}
					} else {
						child.entries_mut().clear();
						for value in &self.#field_ident {
							child.entries_mut().push(::config_traits::kdl::KdlEntry::new(
								<_ as ::config_traits::ConfigValue>::to_kdl_value(value),
							));
						}
						child.clear_children();
					}
				}
			});
		}

		let update_items = if let Some(key) = key {
			let key_ident = syn::Ident::new(key, proc_macro2::Span::call_site());
			quote! {
				for item in &self.#field_ident {
					let key_value = ::config_traits::ConfigValue::to_kdl_value(&item.#key_ident).to_string();
					if let Some(existing) = child_nodes.nodes_mut().iter_mut().find(|n| n.get(#key).map(|v| v.to_string()) == Some(key_value.clone())) {
						::config_traits::ConfigSerialize::apply_to_kdl_node(item, existing)?;
					} else {
						let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
						#apply_doc_comment
						::config_traits::ConfigSerialize::apply_to_kdl_node(item, &mut new_node)?;
						child_nodes.nodes_mut().push(new_node);
					}
				}
			}
		} else {
			quote! {
				for (idx, item) in self.#field_ident.iter().enumerate() {
					if let Some(existing) = child_nodes.nodes_mut().get_mut(idx) {
						::config_traits::ConfigSerialize::apply_to_kdl_node(item, existing)?;
					} else {
						let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
						#apply_doc_comment
						::config_traits::ConfigSerialize::apply_to_kdl_node(item, &mut new_node)?;
						child_nodes.nodes_mut().push(new_node);
					}
				}
			}
		};

		return Some(quote! {
			{
				let child_index = node
					.ensure_children()
					.nodes()
					.iter()
					.position(|n| n.name().value() == #field_name);
				let child = match child_index {
					Some(index) => &mut node.ensure_children().nodes_mut()[index],
					None => {
						let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
						#apply_doc_comment
						node.ensure_children().nodes_mut().push(new_node);
						node.ensure_children().nodes_mut().last_mut().unwrap()
					}
				};
				let child_nodes = child.ensure_children();
				#update_items
			}
		});
	}

	Some(quote! {
		{
			let child_index = node.ensure_children().nodes().iter().position(|n| n.name().value() == #field_name);
				let child = match child_index {
					Some(index) => &mut node.ensure_children().nodes_mut()[index],
					None => {
					let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
					#apply_doc_comment
					node.ensure_children().nodes_mut().push(new_node);
					node.ensure_children().nodes_mut().last_mut().unwrap()
				}
			};
			::config_traits::ConfigSerialize::apply_to_kdl_node(&self.#field_ident, child)?;
		}
	})
}

#[allow(clippy::too_many_lines)]
fn serialize_document_update(
	ty: &syn::Type, field_name: &str, field_ident: &syn::Ident, list_style: Option<ListStyle>,
	list_cutoff: Option<usize>, key: Option<&str>, doc_comment: Option<&str>,
) -> TokenStream {
	let apply_doc_comment = apply_doc_comment(doc_comment, &quote! { new_node });
	if is_option(ty) {
		return quote! {
			{
				let node_index = doc.nodes().iter().position(|n| n.name().value() == #field_name);
				let node = match node_index {
					Some(index) => &mut doc.nodes_mut()[index],
					None => {
						let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
						#apply_doc_comment
						doc.nodes_mut().push(new_node);
						doc.nodes_mut().last_mut().unwrap()
					}
				};
				::config_traits::ConfigSerialize::apply_to_kdl_node(&self.#field_ident, node)?;
			}
		};
	}

	if let Some(_inner) = vec_inner_ty(ty) {
		if key.is_none() {
			let len_ident = syn::Ident::new("__len", proc_macro2::Span::call_site());
			let use_children = list_style_bool(list_style, list_cutoff, &len_ident);
			return quote! {
				{
					let node_index = doc.nodes().iter().position(|n| n.name().value() == #field_name);
					let node = match node_index {
						Some(index) => &mut doc.nodes_mut()[index],
						None => {
							let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
							#apply_doc_comment
							doc.nodes_mut().push(new_node);
							doc.nodes_mut().last_mut().unwrap()
						}
					};
					let #len_ident = self.#field_ident.len();
					let use_children = #use_children;
					if use_children {
						node.entries_mut().clear();
						let dash_children = node.ensure_children();
						dash_children.nodes_mut().retain(|n| n.name().value() == "-");
						for (idx, value) in self.#field_ident.iter().enumerate() {
							let mut node = if idx < dash_children.nodes().len() {
								dash_children.nodes_mut()[idx].clone()
							} else {
								::config_traits::kdl::KdlNode::new("-")
							};
							node.entries_mut().clear();
							node.entries_mut().push(::config_traits::kdl::KdlEntry::new(
								<_ as ::config_traits::ConfigValue>::to_kdl_value(value),
							));
							node.clear_children();
							if idx < dash_children.nodes().len() {
								dash_children.nodes_mut()[idx] = node;
							} else {
								dash_children.nodes_mut().push(node);
							}
						}
					} else {
						node.entries_mut().clear();
						for value in &self.#field_ident {
							node.entries_mut().push(::config_traits::kdl::KdlEntry::new(
								<_ as ::config_traits::ConfigValue>::to_kdl_value(value),
							));
						}
						node.clear_children();
					}
				}
			};
		}

		let update_items = if let Some(key) = key {
			let key_ident = syn::Ident::new(key, proc_macro2::Span::call_site());
			quote! {
				for item in &self.#field_ident {
					let key_value = ::config_traits::ConfigValue::to_kdl_value(&item.#key_ident).to_string();
					if let Some(existing) = children.nodes_mut().iter_mut().find(|n| n.get(#key).map(|v| v.to_string()) == Some(key_value.clone())) {
						::config_traits::ConfigSerialize::apply_to_kdl_node(item, existing)?;
					} else {
						let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
						#apply_doc_comment
						::config_traits::ConfigSerialize::apply_to_kdl_node(item, &mut new_node)?;
						children.nodes_mut().push(new_node);
					}
				}
			}
		} else {
			quote! {
				for (idx, item) in self.#field_ident.iter().enumerate() {
					if let Some(existing) = children.nodes_mut().get_mut(idx) {
						::config_traits::ConfigSerialize::apply_to_kdl_node(item, existing)?;
					} else {
						let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
						#apply_doc_comment
						::config_traits::ConfigSerialize::apply_to_kdl_node(item, &mut new_node)?;
						children.nodes_mut().push(new_node);
					}
				}
			}
		};

		return quote! {
			{
				let node_index = doc.nodes().iter().position(|n| n.name().value() == #field_name);
				let node = match node_index {
					Some(index) => &mut doc.nodes_mut()[index],
					None => {
						let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
						#apply_doc_comment
						doc.nodes_mut().push(new_node);
						doc.nodes_mut().last_mut().unwrap()
					}
				};
				let children = node.ensure_children();
				#update_items
			}
		};
	}

	quote! {
		{
			let node_index = doc.nodes().iter().position(|n| n.name().value() == #field_name);
			let node = match node_index {
				Some(index) => &mut doc.nodes_mut()[index],
				None => {
					let mut new_node = ::config_traits::kdl::KdlNode::new(#field_name);
					#apply_doc_comment
					doc.nodes_mut().push(new_node);
					doc.nodes_mut().last_mut().unwrap()
				}
			};
			::config_traits::ConfigSerialize::apply_to_kdl_node(&self.#field_ident, node)?;
		}
	}
}

fn is_value_type(ty: &syn::Type) -> bool {
	match ty {
		syn::Type::Path(p) => p
			.path
			.get_ident()
			.is_some_and(|id| KDL_VALUE_TYPES.contains(&id.to_string().as_str())),
		_ => false,
	}
}

fn is_bool(ty: &syn::Type) -> bool {
	match ty {
		syn::Type::Path(p) => p.path.is_ident("bool"),
		_ => false,
	}
}

fn vec_inner_ty(ty: &syn::Type) -> Option<&syn::Type> {
	match ty {
		syn::Type::Path(p) if p.path.segments.len() == 1 && p.path.segments[0].ident == "Vec" => {
			match &p.path.segments[0].arguments {
				syn::PathArguments::AngleBracketed(args) => {
					args.args.iter().find_map(|arg| match arg {
						syn::GenericArgument::Type(inner) => Some(inner),
						_ => None,
					})
				}
				_ => None,
			}
		}
		_ => None,
	}
}

fn option_inner_ty(ty: &syn::Type) -> Option<&syn::Type> {
	match ty {
		syn::Type::Path(p)
			if p.path.segments.len() == 1 && p.path.segments[0].ident == "Option" =>
		{
			match &p.path.segments[0].arguments {
				syn::PathArguments::AngleBracketed(args) => {
					args.args.iter().find_map(|arg| match arg {
						syn::GenericArgument::Type(inner) => Some(inner),
						_ => None,
					})
				}
				_ => None,
			}
		}
		_ => None,
	}
}

fn seen_ident(field_ident: &syn::Ident, ty: &syn::Type) -> Option<syn::Ident> {
	if vec_inner_ty(ty).is_some() {
		return None;
	}

	if is_bool(ty) {
		return Some(syn::Ident::new(
			&format!("__seen_{field_ident}"),
			field_ident.span(),
		));
	}

	if is_value_type(ty) || !matches!(ty, syn::Type::Path(_)) {
		return Some(syn::Ident::new(
			&format!("__seen_{field_ident}"),
			field_ident.span(),
		));
	}

	Some(syn::Ident::new(
		&format!("__seen_{field_ident}"),
		field_ident.span(),
	))
}

fn is_required_child(ty: &syn::Type, allow_missing: bool) -> bool {
	if allow_missing {
		return false;
	}

	if is_bool(ty) {
		return false;
	}

	if vec_inner_ty(ty).is_some() {
		return false;
	}

	true
}

fn is_option(ty: &syn::Type) -> bool {
	match ty {
		syn::Type::Path(p)
			if p.path.segments.len() == 1 && p.path.segments[0].ident == "Option" =>
		{
			matches!(
				p.path.segments[0].arguments,
				syn::PathArguments::AngleBracketed(_)
			)
		}
		_ => false,
	}
}

fn default_fallback(
	default_expr: Option<&syn::Expr>, _ty: &syn::Type, is_option: bool, has_default_attr: bool,
) -> TokenStream {
	if let Some(expr) = default_expr {
		if is_option {
			return quote! { Some(#expr) };
		}
		return quote! { #expr };
	}

	if has_default_attr {
		if is_option {
			return quote! { Some(::std::default::Default::default()) };
		}
		return quote! { ::std::default::Default::default() };
	}

	if is_option {
		return quote! { None };
	}

	quote! { ::std::default::Default::default() }
}

fn doc_comment(attrs: &[syn::Attribute]) -> Option<String> {
	let lines = attrs
		.iter()
		.filter(|attr| attr.path().is_ident("doc"))
		.filter_map(|attr| match &attr.meta {
			syn::Meta::NameValue(meta) => match &meta.value {
				syn::Expr::Lit(expr) => match &expr.lit {
					syn::Lit::Str(lit) => Some(lit.value()),
					_ => None,
				},
				_ => None,
			},
			_ => None,
		})
		.collect::<Vec<_>>();

	if lines.is_empty() {
		return None;
	}

	let mut comment = String::new();
	for line in lines {
		let line = line.trim();
		if line.is_empty() {
			comment.push_str("//\n");
		} else {
			comment.push_str("// ");
			comment.push_str(line);
			comment.push('\n');
		}
	}

	Some(comment)
}

fn apply_doc_comment(doc_comment: Option<&str>, node: &TokenStream) -> TokenStream {
	let Some(comment) = doc_comment else {
		return TokenStream::new();
	};

	quote! {
		{
			let mut format = #node.format().cloned().unwrap_or_default();
			format.leading = #comment.to_string();
			#node.set_format(format);
		}
	}
}

// fn ty_to_kdl(ty: &syn::Type) -> TokenStream {
//     let kdl_ty = match ty {
//         syn::Type::Path(path) if path.path.is_ident("String") => quote::quote! { String },
//         _ => abort!(ty, "Unsupported type (for now)"),
//     };
//
//     quote! { ::config_traits::kdl::KdlValue::#kdl_ty }
// }
