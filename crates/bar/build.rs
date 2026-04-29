fn main() {
	// println!("cargo:rerun-if-changed=**/*.blp");
	glib_build_tools::compile_resources(&[".", "./assets/"], "assets/resources.xml", "assets.gresource");
}
