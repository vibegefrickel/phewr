//! A fake terminal that reads the Kitty sequences of a test app.

use std::collections::{BTreeMap, HashMap};

/// One placement: the image, the cursor cell and the cell box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placement {
    pub id: u32,
    /// The cursor as `(column, row)`, 1-based like `CSI row ; column H`.
    pub cell: (u16, u16),
    pub cell_box: Option<(u16, u16)>,
    /// The placement id `p=`. A second place with the same ids moves the placement.
    pub number: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Animation {
    pub gaps: Vec<u32>,
    /// The state of the last `s=` key: 1 stops, 2 waits for frames, 3 runs.
    pub state: Option<u32>,
    /// The loop count of the last `v=` key: 1 loops forever.
    pub loops: Option<u32>,
}

#[derive(Debug)]
pub(crate) struct FakeTerminal {
    /// The stored images by id, in transmit order.
    stored: BTreeMap<u64, (u32, usize)>,
    sequence: u64,
    pub placements: Vec<Placement>,
    /// The placements of ids that the terminal does not store: a black cell each.
    pub black_cells: Vec<u32>,
    pub limit: usize,
    cursor: (u16, u16),
    /// The open synchronized updates: `CSI ? 2026 h` adds one, `CSI ? 2026 l` ends one.
    pub synchronized_depth: i32,
    animations: HashMap<u32, Animation>,
    /// The frames for an image that the terminal does not store: an error each.
    pub orphan_frames: Vec<u32>,
    /// True while a chunked transmission waits for its next chunk (`m=1`).
    in_chunks: bool,
}

impl FakeTerminal {
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            stored: BTreeMap::new(),
            sequence: 0,
            placements: Vec::new(),
            black_cells: Vec::new(),
            limit,
            cursor: (1, 1),
            synchronized_depth: 0,
            animations: HashMap::new(),
            orphan_frames: Vec::new(),
            in_chunks: false,
        }
    }

    pub(crate) fn new() -> Self {
        Self::with_limit(usize::MAX)
    }

    pub(crate) fn stored_bytes(&self) -> usize {
        self.stored.values().map(|(_, bytes)| bytes).sum()
    }

    pub(crate) fn stored_ids(&self) -> Vec<u32> {
        self.stored.values().map(|(id, _)| *id).collect()
    }

    pub(crate) fn stores(&self, id: u32) -> bool {
        self.stored.values().any(|(stored, _)| *stored == id)
    }

    pub(crate) fn animation(&self, id: u32) -> Option<&Animation> {
        self.animations.get(&id).filter(|_| self.stores(id))
    }

    /// Read every sequence in `bytes`.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let Some(start) = rest.iter().position(|&byte| byte == 0x1b) {
            rest = &rest[start..];
            let consumed = if rest.starts_with(b"\x1b_G") {
                self.kitty_command(&rest[3..])
            } else if rest.starts_with(b"\x1b[") {
                self.csi(&rest[2..])
            } else {
                0
            };
            rest = &rest[(consumed + 1).min(rest.len())..];
        }
    }

    /// The CSI `body` after `ESC [`. Returns the length after the `ESC`.
    fn csi(&mut self, body: &[u8]) -> usize {
        let Some(end) = body.iter().position(u8::is_ascii_alphabetic) else {
            return body.len();
        };
        let params = String::from_utf8_lossy(&body[..end]);
        match (params.as_ref(), body[end]) {
            ("?2026", b'h') => self.synchronized_depth += 1,
            ("?2026", b'l') => self.synchronized_depth -= 1,
            (_, b'H') => {
                let mut fields = params.split(';').map(str::parse::<u16>);
                if let (Some(Ok(row)), Some(Ok(column))) = (fields.next(), fields.next()) {
                    self.cursor = (column, row);
                }
            }
            _ => {}
        }
        1 + end + 1
    }

    /// The Kitty command `body` after `ESC _ G`. Returns the length after the `ESC`.
    fn kitty_command(&mut self, body: &[u8]) -> usize {
        let end = body
            .windows(2)
            .position(|pair| pair == b"\x1b\\")
            .unwrap_or(body.len());
        let command = String::from_utf8_lossy(&body[..end]);
        let keys: HashMap<&str, &str> = command
            .split(';')
            .next()
            .unwrap_or_default()
            .split(',')
            .filter_map(|key| key.split_once('='))
            .collect();
        let number = |key: &str| keys.get(key).and_then(|value| value.parse::<u32>().ok());
        let cell_box = match (number("c"), number("r")) {
            (Some(cols), Some(rows)) => Some((cols as u16, rows as u16)),
            _ => None,
        };
        // A further chunk carries only `m=` and, for a frame, `a=f`.
        let further_chunk = self.in_chunks;
        self.in_chunks = number("m") == Some(1);
        if further_chunk {
            return 2 + end + 2;
        }
        match keys.get("a").copied() {
            Some("T") => {
                let id = number("i").unwrap_or(0);
                let bytes = (number("s").unwrap_or(0) * number("v").unwrap_or(0) * 4) as usize;
                self.store(id, bytes);
                self.place(id, cell_box);
            }
            Some("t") => {
                let id = number("i").unwrap_or(0);
                let bytes = (number("s").unwrap_or(0) * number("v").unwrap_or(0) * 4) as usize;
                self.store(id, bytes);
            }
            Some("p") => {
                let (id, placement) = (number("i").unwrap_or(0), number("p"));
                if placement.is_some() {
                    self.placements
                        .retain(|old| (old.id, old.number) != (id, placement));
                }
                self.place(id, cell_box);
                if let Some(last) = self.placements.last_mut().filter(|last| last.id == id) {
                    last.number = placement;
                }
            }
            Some("f") => {
                let id = number("i").unwrap_or(0);
                let bytes = (number("s").unwrap_or(0) * number("v").unwrap_or(0) * 4) as usize;
                self.add_frame(id, bytes, number("z").unwrap_or(40));
            }
            Some("a") => {
                let id = number("i").unwrap_or(0);
                let animation = self.animations.entry(id).or_insert(Animation {
                    gaps: vec![0],
                    state: None,
                    loops: None,
                });
                if let (Some(frame), Some(gap)) = (number("r"), number("z"))
                    && let Some(slot) = animation.gaps.get_mut(frame as usize - 1)
                {
                    *slot = gap;
                }
                animation.state = number("s").or(animation.state);
                animation.loops = number("v").or(animation.loops);
            }
            Some("d") => match keys.get("d").copied() {
                Some("a") => self.placements.clear(),
                Some("A") => {
                    self.placements.clear();
                    self.stored.clear();
                }
                Some("i") => {
                    let id = number("i").unwrap_or(0);
                    let only = number("p");
                    self.placements.retain(|placement| {
                        placement.id != id || only.is_some_and(|p| placement.number != Some(p))
                    });
                }
                Some("I") => {
                    let id = number("i").unwrap_or(0);
                    self.stored.retain(|_, (stored, _)| *stored != id);
                    self.placements.retain(|placement| placement.id != id);
                }
                _ => {}
            },
            _ => {}
        }
        2 + end + 2
    }

    /// Store an image. Above the limit, the oldest unplaced images go first, then the oldest
    /// placed ones. The new image stays.
    fn store(&mut self, id: u32, bytes: usize) {
        self.stored.retain(|_, (stored, _)| *stored != id);
        self.animations.remove(&id);
        self.sequence += 1;
        self.stored.insert(self.sequence, (id, bytes));
        while self.stored_bytes() > self.limit {
            let placed = |id: u32, placements: &[Placement]| {
                placements.iter().any(|placement| placement.id == id)
            };
            let others = self.stored.iter().filter(|(_, (stored, _))| *stored != id);
            let victim = others
                .clone()
                .find(|(_, (stored, _))| !placed(*stored, &self.placements))
                .or_else(|| others.clone().next())
                .map(|(sequence, _)| *sequence);
            let Some(victim) = victim else {
                return;
            };
            self.stored.remove(&victim);
        }
    }

    fn add_frame(&mut self, id: u32, bytes: usize, gap: u32) {
        let Some((_, stored)) = self.stored.values_mut().find(|(stored, _)| *stored == id) else {
            self.orphan_frames.push(id);
            return;
        };
        *stored += bytes;
        let animation = self.animations.entry(id).or_insert(Animation {
            gaps: vec![0],
            state: None,
            loops: None,
        });
        animation.gaps.push(gap);
    }

    fn place(&mut self, id: u32, cell_box: Option<(u16, u16)>) {
        if !self.stores(id) {
            self.black_cells.push(id);
            return;
        }
        self.placements.push(Placement {
            id,
            cell: self.cursor,
            cell_box,
            number: None,
        });
    }
}

#[test]
fn stores_places_and_deletes() {
    let mut terminal = FakeTerminal::new();
    terminal.feed(b"\x1b[?2026h\x1b[3;5H\x1b_Ga=T,q=2,C=1,f=32,s=2,v=1,i=7,m=1;AAAA\x1b\\\x1b_Gm=0;AAAA\x1b\\");
    assert_eq!(terminal.synchronized_depth, 1);
    assert_eq!(terminal.stored_bytes(), 8);
    assert_eq!(
        terminal.placements,
        vec![Placement {
            id: 7,
            cell: (5, 3),
            cell_box: None,
            number: None,
        }]
    );

    terminal.feed(b"\x1b_Ga=d,d=a,q=2\x1b\\\x1b[1;1H\x1b_Ga=p,q=2,C=1,i=7,c=4,r=2\x1b\\\x1b_Ga=p,q=2,C=1,i=8\x1b\\\x1b[?2026l");
    assert_eq!(terminal.placements.len(), 1);
    assert_eq!(terminal.placements[0].cell_box, Some((4, 2)));
    assert_eq!(terminal.black_cells, vec![8]);
    assert_eq!(terminal.synchronized_depth, 0);

    terminal.feed(b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\");
    assert!(!terminal.stores(7));
    assert!(terminal.placements.is_empty());

    terminal.feed(b"\x1b_Ga=T,q=2,C=1,f=32,s=1,v=1,i=9,m=0;AAAA\x1b\\\x1b_Ga=d,d=A,q=2\x1b\\");
    assert!(terminal.stored_ids().is_empty());

    // A transmit without a placement stores, and `d=i` deletes the placement only.
    terminal.feed(b"\x1b_Ga=t,q=2,C=1,f=32,s=1,v=1,i=5,m=0;AAAA\x1b\\");
    assert!(terminal.stores(5));
    assert!(terminal.placements.is_empty());
    terminal.feed(b"\x1b_Ga=p,q=2,C=1,i=5\x1b\\\x1b_Ga=d,d=i,i=5,q=2\x1b\\");
    assert!(terminal.placements.is_empty());
    assert!(terminal.stores(5));
}

#[test]
fn builds_animation_from_frames() {
    let mut terminal = FakeTerminal::new();
    terminal.feed(b"\x1b_Ga=f,q=2,f=32,s=1,v=1,i=5,z=50;AAAA\x1b\\");
    assert_eq!(terminal.orphan_frames, vec![5], "a frame needs its image");

    terminal.feed(b"\x1b_Ga=t,q=2,C=1,f=32,s=1,v=1,i=5,m=0;AAAA\x1b\\");
    terminal.feed(b"\x1b_Ga=f,q=2,f=32,s=1,v=1,i=5,z=50,m=1;AA\x1b\\\x1b_Ga=f,m=0;AA\x1b\\");
    terminal.feed(b"\x1b_Ga=f,q=2,f=32,s=1,v=1,i=5;AAAA\x1b\\");
    terminal.feed(b"\x1b_Ga=a,q=2,i=5,r=1,z=70\x1b\\\x1b_Ga=a,q=2,i=5,s=3,v=1\x1b\\");
    assert_eq!(
        terminal.animation(5),
        Some(&Animation {
            gaps: vec![70, 50, 40],
            state: Some(3),
            loops: Some(1),
        })
    );
    assert_eq!(terminal.stored_bytes(), 12, "the frames add their bytes");
    assert_eq!(terminal.orphan_frames, vec![5]);

    terminal.feed(b"\x1b_Ga=d,d=I,i=5,q=2\x1b\\");
    assert_eq!(terminal.animation(5), None);
}

#[test]
fn evicts_unplaced_images() {
    let mut terminal = FakeTerminal::with_limit(10);
    let transmit = |id: u32| format!("\x1b_Ga=T,q=2,C=1,f=32,s=1,v=1,i={id},m=0;AAAA\x1b\\");
    terminal.feed(format!("{}{}{}", transmit(1), transmit(2), transmit(3)).as_bytes());
    assert_eq!(terminal.stored_bytes(), 8);
    assert_eq!(terminal.stored_ids(), vec![2, 3], "the oldest image went");
    terminal.feed(b"\x1b_Ga=d,d=a,q=2\x1b\\");
    terminal.feed(transmit(4).as_bytes());
    assert_eq!(terminal.stored_ids(), vec![3, 4]);
    terminal.feed(b"\x1b_Ga=p,q=2,C=1,i=2\x1b\\");
    assert_eq!(terminal.black_cells, vec![2]);
}
