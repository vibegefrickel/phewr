use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::path::{Path, PathBuf};

pub(crate) struct PickerEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_parent: bool,
}

pub(crate) struct PickerState {
    pub current_dir: PathBuf,
    pub entries: Vec<PickerEntry>,
    pub filter: String,
    pub filtered_indices: Vec<usize>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub filter_active: bool,
    pub error: Option<String>,
}

impl PickerState {
    pub(crate) fn new(dir: PathBuf) -> Self {
        let mut state = Self {
            current_dir: dir,
            entries: Vec::new(),
            filter: String::new(),
            filtered_indices: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            filter_active: false,
            error: None,
        };
        state.load_dir();
        state
    }

    pub(crate) fn load_dir(&mut self) {
        self.entries.clear();
        self.filter.clear();
        self.error = None;

        if let Some(parent) = self.current_dir.parent() {
            self.entries.push(PickerEntry {
                name: "..".to_owned(),
                path: parent.to_path_buf(),
                is_parent: true,
            });
        }

        match std::fs::read_dir(&self.current_dir) {
            Ok(read_dir) => {
                let mut dirs: Vec<PickerEntry> = Vec::new();
                for entry in read_dir.flatten() {
                    let Ok(ft) = entry.file_type() else {
                        continue;
                    };
                    if !ft.is_dir() {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    dirs.push(PickerEntry {
                        name,
                        path: entry.path(),
                        is_parent: false,
                    });
                }
                dirs.sort_by(|a, b| a.name.cmp(&b.name));
                self.entries.extend(dirs);
            }
            Err(e) => {
                self.error = Some(format!("{}: {}", self.current_dir.display(), e));
            }
        }

        self.rebuild_filter();
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    pub(crate) fn rebuild_filter(&mut self) {
        if self.filter.is_empty() {
            self.filtered_indices = (0..self.entries.len()).collect();
            return;
        }

        let pattern = Pattern::parse(&self.filter, CaseMatching::Ignore, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut buf = Vec::new();

        let mut scored: Vec<(usize, u32)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let haystack = Utf32Str::new(&e.name, &mut buf);
                pattern.score(haystack, &mut matcher).map(|s| (i, s))
            })
            .collect();
        scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        self.filtered_indices = scored.into_iter().map(|(i, _)| i).collect();
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    pub(crate) fn selected(&self) -> Option<&PickerEntry> {
        let &real = self.filtered_indices.get(self.cursor)?;
        self.entries.get(real)
    }

    pub(crate) fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(crate) fn move_down(&mut self) {
        if !self.filtered_indices.is_empty() {
            self.cursor = (self.cursor + 1).min(self.filtered_indices.len() - 1);
        }
    }

    pub(crate) fn move_first(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn move_last(&mut self) {
        self.cursor = self.filtered_indices.len().saturating_sub(1);
    }

    pub(crate) fn move_page_up(&mut self, page: usize) {
        self.cursor = self.cursor.saturating_sub(page.max(1));
    }

    pub(crate) fn move_page_down(&mut self, page: usize) {
        let max = self.filtered_indices.len().saturating_sub(1);
        self.cursor = (self.cursor + page.max(1)).min(max);
    }

    /// Descend into the currently selected directory. Returns the new current path if changed.
    pub(crate) fn enter_selected(&mut self) -> Option<PathBuf> {
        let path = self.selected()?.path.clone();
        self.current_dir.clone_from(&path);
        self.load_dir();
        Some(path)
    }

    /// Go up to the parent directory. Returns the new current path if changed.
    pub(crate) fn ascend(&mut self) -> Option<PathBuf> {
        let parent = self.current_dir.parent()?.to_path_buf();
        self.current_dir.clone_from(&parent);
        self.load_dir();
        Some(parent)
    }

    pub(crate) fn adjust_scroll(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        } else if self.cursor >= self.scroll_offset + visible_height {
            self.scroll_offset = self.cursor - visible_height + 1;
        }
    }

    pub(crate) fn visible_slice(&self, visible_height: usize) -> &[usize] {
        let start = self.scroll_offset;
        let end = (start + visible_height).min(self.filtered_indices.len());
        &self.filtered_indices[start..end]
    }
}

pub(crate) fn initial_picker_dir(images: &[PathBuf]) -> PathBuf {
    let candidate = images
        .first()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    canonicalize_or_self(&candidate)
}

pub(crate) fn canonicalize_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::{PickerState, canonicalize_or_self, initial_picker_dir};
    use std::fs;
    use std::path::PathBuf;

    fn synthetic_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("viewr-picker-test-{name}-{}", std::process::id()));
        if root.exists() {
            fs::remove_dir_all(&root).unwrap();
        }
        fs::create_dir_all(&root).unwrap();
        fs::canonicalize(&root).unwrap()
    }

    #[test]
    fn new_loads_parent_and_visible_directories_sorted() {
        let root = synthetic_dir("load");
        let alpha = root.join("alpha");
        let beta = root.join("beta");
        let hidden = root.join(".hidden");
        let file = root.join("file.txt");
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        fs::create_dir_all(&hidden).unwrap();
        fs::write(&file, b"").unwrap();

        let picker = PickerState::new(root.clone());
        assert_eq!(picker.entries.len(), 3);
        assert!(picker.entries[0].is_parent);
        assert_eq!(picker.entries[1].name, "alpha");
        assert_eq!(picker.entries[2].name, "beta");
        assert_eq!(picker.filtered_indices, vec![0, 1, 2]);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_dir_reports_error_for_missing_path() {
        let root = synthetic_dir("error");
        let missing = root.join("missing");
        let mut picker = PickerState::new(root.clone());
        picker.current_dir = missing;
        picker.load_dir();
        assert!(picker.error.is_some());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filter_scores_and_reorders() {
        let root = synthetic_dir("filter");
        fs::create_dir_all(root.join("apple")).unwrap();
        fs::create_dir_all(root.join("banana")).unwrap();
        let mut picker = PickerState::new(root.clone());
        picker.filter = "ana".to_owned();
        picker.rebuild_filter();

        assert_eq!(picker.filtered_indices.len(), 1);
        assert_eq!(picker.entries[picker.filtered_indices[0]].name, "banana");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_filter_preserves_order_and_cursor() {
        let root = synthetic_dir("empty-filter");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        let mut picker = PickerState::new(root.clone());
        picker.cursor = 2;
        picker.filter = String::new();
        picker.rebuild_filter();

        assert_eq!(picker.filtered_indices, vec![0, 1, 2]);
        assert_eq!(picker.cursor, 2);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cursor_navigation_stays_in_filtered_bounds() {
        let root = synthetic_dir("cursor");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::create_dir_all(root.join("c")).unwrap();
        let mut picker = PickerState::new(root.clone());

        assert_eq!(picker.selected().map(|e| &e.name), Some(&"..".to_owned()));
        picker.move_down();
        assert_eq!(picker.selected().map(|e| &e.name), Some(&"a".to_owned()));
        picker.move_last();
        assert_eq!(picker.selected().map(|e| &e.name), Some(&"c".to_owned()));
        picker.move_down();
        assert_eq!(picker.selected().map(|e| &e.name), Some(&"c".to_owned()));
        picker.move_up();
        assert_eq!(picker.selected().map(|e| &e.name), Some(&"b".to_owned()));
        picker.move_first();
        assert_eq!(picker.selected().map(|e| &e.name), Some(&"..".to_owned()));
        picker.move_up();
        assert_eq!(picker.selected().map(|e| &e.name), Some(&"..".to_owned()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn page_navigation_jumps_and_clamps() {
        let root = synthetic_dir("page");
        for i in 0..5 {
            fs::create_dir_all(root.join(format!("dir{i}"))).unwrap();
        }
        let mut picker = PickerState::new(root.clone());

        picker.move_page_down(2);
        assert_eq!(picker.cursor, 2);
        picker.move_page_down(10);
        assert_eq!(picker.cursor, 5);
        picker.move_page_up(10);
        assert_eq!(picker.cursor, 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn enter_descends_and_ascend_goes_up() {
        let root = synthetic_dir("navigate");
        let child = root.join("child");
        fs::create_dir_all(&child).unwrap();
        let mut picker = PickerState::new(root.clone());
        picker.cursor = 1;

        let new_path = picker.enter_selected();
        assert_eq!(new_path, Some(fs::canonicalize(&child).unwrap()));
        assert_eq!(picker.current_dir, fs::canonicalize(&child).unwrap());

        let parent = picker.ascend();
        assert_eq!(parent, Some(root.clone()));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ascend_from_root_returns_none() {
        let root = synthetic_dir("root");
        let mut picker = PickerState::new(root.clone());
        picker.current_dir = PathBuf::from("/");

        assert!(picker.ascend().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    fn empty_picker() -> PickerState {
        PickerState {
            current_dir: PathBuf::from("."),
            entries: Vec::new(),
            filter: String::new(),
            filtered_indices: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            filter_active: false,
            error: None,
        }
    }

    #[test]
    fn adjust_scroll_keeps_cursor_visible() {
        let mut picker = empty_picker();
        picker.filtered_indices = (0..10).collect();
        picker.cursor = 0;
        picker.scroll_offset = 0;
        picker.adjust_scroll(5);
        assert_eq!(picker.scroll_offset, 0);

        picker.cursor = 7;
        picker.adjust_scroll(5);
        assert_eq!(picker.scroll_offset, 3);

        picker.cursor = 2;
        picker.adjust_scroll(5);
        assert_eq!(picker.scroll_offset, 2);
    }

    #[test]
    fn visible_slice_respects_scroll_and_height() {
        let mut picker = empty_picker();
        picker.filtered_indices = (0..10).collect();
        picker.scroll_offset = 3;

        let slice = picker.visible_slice(4);
        assert_eq!(slice, &[3, 4, 5, 6]);
    }

    #[test]
    fn adjust_scroll_zero_height_is_no_op() {
        let mut picker = empty_picker();
        picker.filtered_indices = (0..10).collect();
        picker.cursor = 5;
        picker.scroll_offset = 2;
        picker.adjust_scroll(0);

        assert_eq!(picker.cursor, 5);
        assert_eq!(picker.scroll_offset, 2);
    }

    #[test]
    fn initial_picker_dir_uses_parent_or_dot() {
        let images = vec![PathBuf::from("/tmp/photo.png")];
        assert_eq!(
            initial_picker_dir(&images),
            canonicalize_or_self(PathBuf::from("/tmp").as_path())
        );
        assert_eq!(
            initial_picker_dir(&[]),
            canonicalize_or_self(PathBuf::from(".").as_path())
        );
    }

    #[test]
    fn canonicalize_or_self_falls_back_on_missing_path() {
        let missing = PathBuf::from("/viewr-does-not-exist-xyz");
        assert_eq!(canonicalize_or_self(&missing), missing);
    }
}
