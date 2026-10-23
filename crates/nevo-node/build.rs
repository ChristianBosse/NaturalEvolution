fn main() {
    // The node's own screen is compiled only with `--features screen` (D-062).
    #[cfg(feature = "screen")]
    {
        let config = slint_build::CompilerConfiguration::new().with_style("fluent-light".into());
        slint_build::compile_with_config("ui/screen.slint", config).expect("ui/screen.slint compiles");
    }
}
