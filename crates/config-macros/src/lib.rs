use proc_macro::TokenStream;
use proc_macro_error2::proc_macro_error;
use syn::DeriveInput;

mod config;

#[proc_macro_error]
#[proc_macro_derive(Config, attributes(config))]
pub fn config_macro(input: TokenStream) -> TokenStream {
	let input = syn::parse_macro_input!(input as DeriveInput);

	match config::generate_config(input) {
		Ok(tokens) => tokens.into(),
		Err(e) => e.to_compile_error().into(),
	}
}

#[proc_macro_error]
#[proc_macro_derive(ConfigFile, attributes(config))]
pub fn config_file_macro(input: TokenStream) -> TokenStream {
	let input = syn::parse_macro_input!(input as DeriveInput);

	match config::generate_config_file(input) {
		Ok(tokens) => tokens.into(),
		Err(e) => e.to_compile_error().into(),
	}
}

#[proc_macro_error]
#[proc_macro_derive(ConfigSerialize, attributes(config))]
pub fn config_serialize_macro(input: TokenStream) -> TokenStream {
	let input = syn::parse_macro_input!(input as DeriveInput);

	match config::generate_config_serialize(input) {
		Ok(tokens) => tokens.into(),
		Err(e) => e.to_compile_error().into(),
	}
}

#[proc_macro_error]
#[proc_macro_derive(ConfigFileSerialize, attributes(config))]
pub fn config_file_serialize_macro(input: TokenStream) -> TokenStream {
	let input = syn::parse_macro_input!(input as DeriveInput);

	match config::generate_config_file_serialize(input) {
		Ok(tokens) => tokens.into(),
		Err(e) => e.to_compile_error().into(),
	}
}
