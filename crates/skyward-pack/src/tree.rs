//! The phone's device tree and the overlays a recovery boot is given on top of
//! it, which is where the card tray's lines are described.
//!
//! The vendor's tree for this phone's last revision names the tray's detect
//! line active high and leaves the line without a pull, while the board pulls
//! the line up and grounds it as a card goes in: read high, it says the tray is
//! empty for as long as the phone runs and the kernel never looks at the card.
//! Both are words of the tree, so a build that is to see a card puts them right
//! in the recovery's own copies, leaving the installed system's alone.

use anyhow::{bail, ensure, Context, Result};

/// The magic of a device tree overlay image, as Google's `dt_table_header`
/// begins.
const OVERLAY_MAGIC: u32 = 0xd7b7_ab1e;

/// The magic of one flattened device tree.
const FDT_MAGIC: u32 = 0xd00d_feed;

/// The property naming the line a card's coming and going is read on.
const DETECT: &str = "cd-gpios";

/// The property that says how a line is pulled, and the value that leaves it
/// floating.
const PULL: (&str, &str) = ("bias-disable", "bias-pull-up");

/// The property that names the pins of a group, by them.
const PINS: &str = "pins";

/// The group the tray's detect line belongs to, whose configuration says how
/// the line is pulled.
const GROUP: &str = "config";

/// The line's level that means a card is in: the board grounds the line as one
/// goes in, and pulls it up while the tray is empty.
const ACTIVE_LOW: u32 = 1;

/// The parts of one flattened device tree that a patch reads and writes.
struct Tree<'a> {
    fdt: &'a mut [u8],
    strings_at: usize,
    strings_len: usize,
}

/// A property, as it lies in the tree's structure block.
struct Prop {
    /// Where the name's offset in the string block is written: renaming the
    /// property is a word written there, the strings being kept whole.
    name_at: usize,
    /// Where the value begins, which a patch of the same size writes over.
    value_at: usize,
    name: String,
    value: Vec<u8>,
}

impl<'a> Tree<'a> {
    fn new(fdt: &'a mut [u8]) -> Result<Tree<'a>> {
        ensure!(be32(fdt, 0)? == FDT_MAGIC, "not a flattened device tree");
        Ok(Tree { strings_at: be32(fdt, 12)? as usize, strings_len: be32(fdt, 32)? as usize, fdt })
    }

    /// The nodes of the tree in the order it holds them, each with the
    /// properties it names.
    fn nodes(&self) -> Result<Vec<(Vec<String>, Vec<Prop>)>> {
        let strings = self.strings().to_vec();
        let mut at = be32(self.fdt, 8)? as usize;
        let end = at + be32(self.fdt, 36)? as usize;
        let mut path: Vec<String> = Vec::new();
        let mut out: Vec<(Vec<String>, Vec<Prop>)> = Vec::new();
        while at + 4 <= end {
            let token = be32(self.fdt, at)?;
            at += 4;
            match token {
                1 => {
                    let name = self.fdt.get(at..).context("the node's name runs past the tree")?;
                    let len = name.iter().position(|b| *b == 0).context("a node's name has no end")?;
                    path.push(String::from_utf8_lossy(&name[..len]).into_owned());
                    out.push((path.clone(), Vec::new()));
                    at = (at + len + 1 + 3) & !3;
                }
                2 => {
                    path.pop();
                }
                3 => {
                    let len = be32(self.fdt, at)? as usize;
                    let name_at = at + 4;
                    at += 8;
                    let value_at = at;
                    let name = strings.get(be32(self.fdt, name_at)? as usize..).and_then(|s| s.split(|b| *b == 0).next());
                    let name = String::from_utf8_lossy(name.context("a property's name is past the strings")?).into_owned();
                    let value = self.fdt.get(at..at + len).context("a property's value runs past the tree")?.to_vec();
                    at = (at + len + 3) & !3;
                    if let Some((_, props)) = out.last_mut() {
                        props.push(Prop { name_at, value_at, name, value });
                    }
                }
                4 => {}
                9 => break,
                other => bail!("unknown token {other:#x} in the tree's structure"),
            }
        }
        Ok(out)
    }

    fn strings(&self) -> &[u8] {
        &self.fdt[self.strings_at..self.strings_at + self.strings_len]
    }

    /// Where `word` is written in the string block, as the trees of a phone
    /// hold every bias their groups may ask for.
    fn word_at(&self, word: &str) -> Result<u32> {
        let strings = self.strings();
        let mut at = 0;
        while at < strings.len() {
            let end = at + strings[at..].iter().position(|b| *b == 0).context("a string has no end")?;
            if &strings[at..end] == word.as_bytes() {
                return Ok(at as u32);
            }
            at = end + 1;
        }
        bail!("the tree says no {word:?}")
    }
}

fn be32(blob: &[u8], at: usize) -> Result<u32> {
    let cell = blob.get(at..at + 4).with_context(|| format!("no cell at {at}"))?;
    Ok(u32::from_be_bytes(cell.try_into().expect("four bytes")))
}

/// Every overlay of `dtbo`, as its place in the image and its size. An entry
/// is the tree's size and then its offset, the entry table following the
/// header, and an entry naming no tree is left out.
fn overlays(dtbo: &[u8]) -> Result<Vec<(usize, usize)>> {
    ensure!(be32(dtbo, 0)? == OVERLAY_MAGIC, "not a device tree overlay image");
    let (entry_size, count, entries_at) = (be32(dtbo, 12)? as usize, be32(dtbo, 16)? as usize, be32(dtbo, 20)? as usize);
    ensure!(entry_size >= 8 && count > 0, "the overlay image holds no entries");
    let mut out = Vec::new();
    for k in 0..count {
        let at = entries_at + k * entry_size;
        let entry = dtbo.get(at..at + 8).with_context(|| format!("the entry at {at} runs past the image"))?;
        let (size, offset) = (be32(entry, 0)? as usize, be32(entry, 4)? as usize);
        if size == 0 {
            continue;
        }
        ensure!(offset + size <= dtbo.len(), "the overlay at {offset} runs past the image");
        out.push((offset, size));
    }
    Ok(out)
}

/// The overlays' word for `pin` read as the level a card in the tray gives:
/// the ones that call it active high are made active low, every other word of
/// them kept as it is, the values being the same size either way. Gives the
/// overlays changed.
pub fn detect_active_low(dtbo: &[u8], pin: u32) -> Result<(Vec<u8>, usize)> {
    let mut out = dtbo.to_vec();
    let mut changed = 0;
    for (at, size) in overlays(&out)? {
        let tree = Tree::new(&mut out[at..at + size])?;
        let wanted = pin.to_be_bytes().to_vec();
        for (_, props) in tree.nodes()? {
            for prop in props {
                // A cell for the phandle the overlay fixes up as it is
                // applied, the line's number, and the level that means a card
                // is in.
                if prop.name != DETECT || prop.value.len() != 12 || prop.value[4..8] != wanted {
                    continue;
                }
                let flags = prop.value_at + 8;
                if be32(tree.fdt, flags)? != ACTIVE_LOW {
                    tree.fdt[flags..flags + 4].copy_from_slice(&ACTIVE_LOW.to_be_bytes());
                    changed += 1;
                }
            }
        }
    }
    Ok((out, changed))
}

/// The trees of a boot image's device tree section, which holds one tree per
/// board revision the image was built for.
fn trees(dtb: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= dtb.len() {
        let (magic, size) = (be32(dtb, at).unwrap_or(0), be32(dtb, at + 4).unwrap_or(0) as usize);
        if magic != FDT_MAGIC || size < 40 || at + size > dtb.len() {
            break;
        }
        out.push((at, size));
        at += (size + 3) & !3;
    }
    out
}

/// The group holding `pin` is given a pull up where the tree leaves it
/// floating, so that the line reads the same at every boot. Gives the groups
/// changed.
pub fn detect_pull_up(dtb: &[u8], pin: u32) -> Result<(Vec<u8>, usize)> {
    let mut out = dtb.to_vec();
    let mut changed = 0;
    let named = format!("gpio{pin}");
    for (at, size) in trees(&out) {
        let fdt = &mut out[at..at + size];
        let tree = Tree::new(fdt)?;
        let pull_up = tree.word_at(PULL.1)?;
        let nodes = tree.nodes()?;
        for (path, props) in nodes {
            let Some(()) = path.last().filter(|name| *name == GROUP).map(|_| ()) else { continue };
            let holds_pin = props.iter().any(|p| p.name == PINS && String::from_utf8_lossy(&p.value).trim_end_matches('\0') == named);
            let floating = props.iter().any(|p| p.name == PULL.0);
            if !holds_pin || !floating {
                continue;
            }
            for prop in props.iter().filter(|p| p.name == PULL.0) {
                tree.fdt[prop.name_at..prop.name_at + 4].copy_from_slice(&pull_up.to_be_bytes());
                changed += 1;
            }
        }
    }
    Ok((out, changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A node of a test's tree: its path from the root, and the properties it
    /// holds, each by name and value.
    type Node<'a> = (Vec<&'a str>, Vec<(&'a str, Vec<u8>)>);

    /// Builds a whole flattened tree: `nodes` are the nodes, and the string
    /// block carries every word they name plus the pulls a phone's trees hold
    /// anyway.
    fn fdt(nodes: &[Node]) -> Vec<u8> {
        let mut strings = Vec::new();
        let word = |word: &str, strings: &mut Vec<u8>| {
            if let Some(at) = find(strings, word) {
                return at;
            }
            let at = strings.len();
            strings.extend_from_slice(word.as_bytes());
            strings.push(0);
            at as u32
        };
        for name in ["bias-disable", "bias-pull-up", "bias-pull-down"] {
            word(name, &mut strings);
        }
        let mut structure = Vec::new();
        structure.extend_from_slice(&1u32.to_be_bytes());
        structure.push(0);
        structure.extend_from_slice(&[0; 3]);
        for (path, props) in nodes {
            for name in path {
                structure.extend_from_slice(&1u32.to_be_bytes());
                structure.extend_from_slice(name.as_bytes());
                structure.push(0);
                while structure.len() % 4 != 0 {
                    structure.push(0);
                }
            }
            for (name, value) in props {
                let at = word(name, &mut strings);
                structure.extend_from_slice(&3u32.to_be_bytes());
                structure.extend_from_slice(&(value.len() as u32).to_be_bytes());
                structure.extend_from_slice(&at.to_be_bytes());
                structure.extend_from_slice(value);
                while structure.len() % 4 != 0 {
                    structure.push(0);
                }
            }
            for _ in path {
                structure.extend_from_slice(&2u32.to_be_bytes());
            }
        }
        structure.extend_from_slice(&9u32.to_be_bytes());
        let header = 40 + 16;
        let strings_at = header + structure.len();
        let total = strings_at + strings.len();
        let mut out = Vec::new();
        for cell in [FDT_MAGIC, total as u32, header as u32, strings_at as u32, 40, 17, 16, 0, strings.len() as u32, structure.len() as u32] {
            out.extend_from_slice(&cell.to_be_bytes());
        }
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(&structure);
        out.extend_from_slice(&strings);
        out
    }

    fn find(haystack: &[u8], word: &str) -> Option<u32> {
        let mut at = 0;
        while at < haystack.len() {
            let end = at + haystack[at..].iter().position(|b| *b == 0)?;
            if &haystack[at..end] == word.as_bytes() {
                return Some(at as u32);
            }
            at = end + 1;
        }
        None
    }

    /// An overlay image holding `fdt` as its only entry.
    fn overlay(fdt: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for cell in [OVERLAY_MAGIC, (64 + fdt.len()) as u32, 32, 32, 1, 32, 4096, 0] {
            out.extend_from_slice(&cell.to_be_bytes());
        }
        out.extend_from_slice(&(fdt.len() as u32).to_be_bytes());
        out.extend_from_slice(&64u32.to_be_bytes());
        out.extend_from_slice(&[0; 24]);
        out.extend_from_slice(fdt);
        out
    }

    /// The tree an overlay fragment naming the tray's detect line is.
    fn detect_tree(flags: u32) -> Vec<u8> {
        let mut value = 0xffff_ffffu32.to_be_bytes().to_vec();
        value.extend_from_slice(&77u32.to_be_bytes());
        value.extend_from_slice(&flags.to_be_bytes());
        fdt(&[(vec!["fragment@1", "__overlay__"], vec![(DETECT, value)])])
    }

    /// The tree a phone's pin group is: the tray's line, held by a group whose
    /// configuration leaves the line floating.
    fn pull_tree(config: &str) -> Vec<u8> {
        fdt(&[
            (vec!["pinctrl@f000000", "storage_cd", "mux"], vec![("pins", b"gpio77\0".to_vec()), ("function", b"gpio\0".to_vec())]),
            (vec!["pinctrl@f000000", "storage_cd", "config"], vec![("pins", b"gpio77\0".to_vec()), (config, Vec::new())]),
        ])
    }

    /// The properties of the node at `path`, as a patch left them.
    fn props_of(tree: &[u8], path: &str) -> Vec<(String, Vec<u8>)> {
        let mut copy = tree.to_vec();
        let tree = Tree::new(&mut copy).unwrap();
        tree.nodes()
            .unwrap()
            .into_iter()
            .find(|(p, _)| p.join("/").trim_start_matches('/') == path)
            .map(|(_, props)| props.into_iter().map(|p| (p.name, p.value)).collect())
            .unwrap_or_default()
    }

    #[test]
    fn a_detect_line_named_active_high_is_named_active_low() {
        let image = overlay(&detect_tree(0));
        let (fixed, changed) = detect_active_low(&image, 77).unwrap();
        assert_eq!(changed, 1);
        let (again, none) = detect_active_low(&fixed, 77).unwrap();
        assert_eq!((none, again), (0, fixed), "there is nothing left to change");
    }

    #[test]
    fn a_detect_line_of_another_pin_is_left_alone() {
        let image = overlay(&detect_tree(0));
        let (same, none) = detect_active_low(&image, 78).unwrap();
        assert_eq!((none, same), (0, image));
    }

    #[test]
    fn the_trays_line_is_pulled_up_where_the_tree_leaves_it_floating() {
        let tree = pull_tree("bias-disable");
        let (fixed, changed) = detect_pull_up(&tree, 77).unwrap();
        assert_eq!(changed, 1);
        let config = props_of(&fixed, "pinctrl@f000000/storage_cd/config");
        assert!(config.iter().any(|(name, _)| name == "bias-pull-up"), "the group is pulled up now");
        assert!(!config.iter().any(|(name, _)| name == "bias-disable"), "and is no longer left floating");
    }

    #[test]
    fn a_group_of_another_pin_is_left_alone() {
        let tree = pull_tree("bias-disable");
        let (same, none) = detect_pull_up(&tree, 78).unwrap();
        assert_eq!((none, same), (0, tree));
        let pulled = pull_tree("bias-pull-down");
        let (same, none) = detect_pull_up(&pulled, 77).unwrap();
        assert_eq!((none, same), (0, pulled), "a group pulled down is its own choice, not a floating line");
    }

    #[test]
    fn an_image_that_is_no_overlay_image_is_refused() {
        assert!(detect_active_low(b"not an overlay image", 77).is_err());
    }
}
