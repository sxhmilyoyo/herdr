use super::App;

impl App {
    pub(crate) fn set_host_terminal_appearance_state(
        &mut self,
        appearance: Option<crate::terminal_theme::HostAppearance>,
        explicit: bool,
    ) -> bool {
        if self.state.host_terminal_appearance == appearance
            && self.state.host_terminal_appearance_explicit == explicit
        {
            return false;
        }
        self.state.host_terminal_appearance = appearance;
        self.state.host_terminal_appearance_explicit = explicit;
        self.apply_host_terminal_appearance_to_panes();
        self.refresh_effective_app_theme()
    }

    pub(crate) fn set_host_terminal_theme(
        &mut self,
        theme: crate::terminal_theme::TerminalTheme,
    ) -> bool {
        if theme == self.state.host_terminal_theme {
            return false;
        }
        // A host reports an appearance change before it answers the colour query that follows it,
        // so the appearance reaches panes first and the background lands later. A child that
        // re-reads its background when the report arrives would otherwise keep the previous one
        // until something else made it ask again.
        let background_changed = theme.background != self.state.host_terminal_theme.background;
        self.state.host_terminal_theme = theme;
        self.apply_host_terminal_theme_to_panes();
        if background_changed {
            self.reassert_host_terminal_appearance_to_panes();
        }
        true
    }

    pub(super) fn refresh_effective_app_theme(&mut self) -> bool {
        let (palette, theme_name) = super::resolve_effective_theme(
            &self.state.theme_runtime,
            self.state.host_terminal_appearance,
        );
        if self.state.theme_name == theme_name && self.state.palette == palette {
            return false;
        }
        self.state.theme_name = theme_name;
        self.state.palette = palette;
        self.render_dirty.request_generic();
        self.render_notify.notify_one();
        true
    }

    fn apply_host_terminal_appearance_to_panes(&self) {
        for runtime in self.terminal_runtimes.values() {
            runtime.apply_host_terminal_appearance(self.state.host_terminal_appearance);
        }
    }

    /// Re-notify subscribed children now that the default background matches the appearance.
    ///
    /// Panes with no known appearance, and children that never enabled the report mode, produce
    /// nothing here.
    fn reassert_host_terminal_appearance_to_panes(&self) {
        for runtime in self.terminal_runtimes.values() {
            runtime.reassert_host_terminal_appearance();
        }
    }

    fn apply_host_terminal_theme_to_panes(&self) {
        for runtime in self.terminal_runtimes.values() {
            runtime.apply_host_terminal_theme(self.state.host_terminal_theme);
        }

        self.render_dirty.request_generic();
        self.render_notify.notify_one();
    }
}
