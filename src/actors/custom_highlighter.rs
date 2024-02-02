use tailspin::{
    cli::{self, Cli},
    highlight_processor::HighlightProcessor,
    highlighters,
    theme::{self, processed::Theme},
    theme_io,
};

pub fn build_highlighter(theme: Theme, cli: Cli) -> HighlightProcessor {
    let highlighter = highlighters::Highlighters::new(&theme, &cli);
    let highlight_processor = HighlightProcessor::new(highlighter);

    highlight_processor
}

pub struct CustomHighlighter {
    // ...
}

impl CustomHighlighter {
    pub fn build() -> HighlightProcessor {
        let cli = cli::get_args_or_exit_early();
        let theme = theme_io::load_theme(cli.config_path.clone());
        let processed_theme = theme::mapper::map_or_exit_early(theme);

        build_highlighter(processed_theme, cli)
    }
}
