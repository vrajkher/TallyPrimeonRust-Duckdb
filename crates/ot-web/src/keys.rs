//! Tally-style keyboard shortcuts, as a pure mapping so it can be tested natively.

/// What a key press asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// F4
    Contra,
    /// F5
    Payment,
    /// F6
    Receipt,
    /// F7
    Journal,
    /// F8
    Sales,
    /// F9
    Purchase,
    /// Alt+D
    DayBook,
    /// Alt+A: the chart of accounts.
    Accounts,
    /// `/`: focus the list search.
    Search,
    /// Ctrl+A: accept (save) the open form, as in Tally.
    Save,
    /// Alt+C: create a new record.
    Create,
    /// Esc
    Back,
    /// Page Down
    NextPage,
    /// Page Up
    PrevPage,
}

/// The modifier keys held with a key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

/// Maps a `KeyboardEvent.key` to an action. `typing` is true while focus is in a text
/// field, where plain keys such as `/` must type normally.
pub fn action(key: &str, mods: Modifiers, typing: bool) -> Option<Action> {
    let plain = !mods.ctrl && !mods.alt && !mods.meta && !mods.shift;
    match key {
        "F4" if plain => Some(Action::Contra),
        "F5" if plain => Some(Action::Payment),
        "F6" if plain => Some(Action::Receipt),
        "F7" if plain => Some(Action::Journal),
        "F8" if plain => Some(Action::Sales),
        "F9" if plain => Some(Action::Purchase),
        "Escape" if plain => Some(Action::Back),
        "PageDown" if plain && !typing => Some(Action::NextPage),
        "PageUp" if plain && !typing => Some(Action::PrevPage),
        "/" if plain && !typing => Some(Action::Search),
        "a" | "A" if (mods.ctrl || mods.meta) && !mods.alt && !mods.shift => Some(Action::Save),
        "d" | "D" if mods.alt && !mods.ctrl && !mods.meta => Some(Action::DayBook),
        "a" | "A" if mods.alt && !mods.ctrl && !mods.meta => Some(Action::Accounts),
        "c" | "C" if mods.alt && !mods.ctrl && !mods.meta => Some(Action::Create),
        _ => None,
    }
}

/// The voucher kind a function key opens, for the classic entry screen.
pub fn voucher_kind(action: Action) -> Option<&'static str> {
    match action {
        Action::Contra => Some("contra"),
        Action::Payment => Some("payment"),
        Action::Receipt => Some("receipt"),
        Action::Journal => Some("journal"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };

    #[test]
    fn function_keys_follow_tally() {
        assert_eq!(action("F4", NONE, false), Some(Action::Contra));
        assert_eq!(action("F5", NONE, true), Some(Action::Payment));
        assert_eq!(action("F6", NONE, false), Some(Action::Receipt));
        assert_eq!(action("F7", NONE, false), Some(Action::Journal));
        assert_eq!(action("F8", NONE, false), Some(Action::Sales));
        assert_eq!(action("F9", NONE, false), Some(Action::Purchase));
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        assert_eq!(action("F5", shift, false), None);
        assert_eq!(voucher_kind(Action::Payment), Some("payment"));
        assert_eq!(voucher_kind(Action::Sales), None);
    }

    #[test]
    fn accept_create_and_navigation() {
        let ctrl = Modifiers { ctrl: true, ..NONE };
        let alt = Modifiers { alt: true, ..NONE };
        assert_eq!(action("a", ctrl, true), Some(Action::Save));
        assert_eq!(action("d", alt, false), Some(Action::DayBook));
        assert_eq!(action("a", alt, true), Some(Action::Accounts));
        assert_eq!(action("C", alt, true), Some(Action::Create));
        assert_eq!(action("Escape", NONE, true), Some(Action::Back));
        assert_eq!(action("a", NONE, false), None);
    }

    #[test]
    fn plain_keys_type_normally_in_fields() {
        assert_eq!(action("/", NONE, false), Some(Action::Search));
        assert_eq!(action("/", NONE, true), None);
        assert_eq!(action("PageDown", NONE, false), Some(Action::NextPage));
        assert_eq!(action("PageDown", NONE, true), None);
    }
}
