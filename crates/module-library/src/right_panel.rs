//! Library right panel (plan §7.6): Metadata (a read-only EXIF readout
//! of the active photo plus the editable IPTC core), Keywording (entry
//! with autocomplete + the applied keywords as removable chips), the
//! Keyword List tree, and the XMP footer. Everything read from the
//! catalog is cached in `RightPanelState` and only re-queried when the
//! selection moves or `LibraryData::metadata_dirty` changes — never
//! per frame for an unchanged selection.

use std::collections::{BTreeMap, BTreeSet};

use viberoom_core::ids::{KeywordId, PhotoId};
use viberoom_core::settings::XmpAutoWrite;
use viberoom_services::command::{AddKeywords, IptcField, RemoveKeywords, SetIptc};
use viberoom_services::repo::{self, KeywordRow};
use viberoom_services::sidecar::SaveXmpJob;
use viberoom_shell::AppCx;

use crate::photos::LibraryData;
use crate::shortcuts;

/// Up to this many autocomplete suggestions under the Keywording entry.
const MAX_SUGGESTIONS: usize = 8;

const IPTC_FIELDS: [IptcField; 4] = [
    IptcField::Title,
    IptcField::Caption,
    IptcField::Creator,
    IptcField::Copyright,
];

/// The Keywording entry's widget id, so Ctrl+K (plan §10.3) can focus it
/// from `shortcuts::handle` via egui's memory.
fn keyword_entry_id() -> egui::Id {
    egui::Id::new("library_keyword_entry")
}

/// All right-panel UI state. The catalog reads live in `cache` and are
/// refreshed only when the cache key changes (see [`show`]).
#[derive(Debug)]
pub struct RightPanelState {
    /// Set by Ctrl+K in `shortcuts::handle`, consumed by [`show`] to
    /// focus the Keywording entry. The right panel is painted before
    /// `center()` runs (plan §10.1's layout order), so the focus lands
    /// the frame after the keypress.
    pub focus_keyword_entry: bool,
    /// The Keywording entry's text.
    keyword_entry: String,
    /// Autocomplete suggestions for the entry, plus the entry text
    /// they were computed from — recomputed only when the user types.
    suggestions: Vec<String>,
    suggestion_source: String,
    /// One draft buffer per IPTC field (same order as `IPTC_FIELDS`).
    iptc: [IptcDraft; 4],
    cache: Option<PanelCache>,
    /// Bumped when the "Include on export" context-menu toggle runs —
    /// it's the sole mutation that doesn't go through `apply_command`
    /// (it isn't a photo change), so it publishes no `CatalogEvent` to
    /// invalidate the cache with.
    keyword_list_version: u64,
}

impl Default for RightPanelState {
    fn default() -> Self {
        Self {
            focus_keyword_entry: false,
            keyword_entry: String::new(),
            suggestions: Vec::new(),
            suggestion_source: String::new(),
            iptc: std::array::from_fn(|_| IptcDraft::default()),
            cache: None,
            keyword_list_version: 0,
        }
    }
}

/// One IPTC field's editable text. `loaded` is what the draft was seeded
/// from (the empty string for a `<mixed>` field): a commit only fires
/// when the user actually moved the text off it.
#[derive(Debug, Default)]
struct IptcDraft {
    text: String,
    loaded: String,
}

/// A field's value across the current targets.
#[derive(Debug)]
enum FieldValues {
    /// Every target has this value (possibly `None`).
    Uniform(Option<String>),
    /// The targets disagree — the field shows a `<mixed>` placeholder.
    Mixed,
}

/// The tri-state of "this keyword is on the targets" (Keyword List).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tri {
    Unchecked,
    Mixed,
    Checked,
}

/// Everything the panels read from the catalog, re-queried only when the
/// key changes.
#[derive(Debug)]
struct PanelCache {
    key: CacheKey,
    /// The photos edits apply to: the selection if non-empty, else the
    /// active photo (the same rule as `shortcuts::targets`).
    targets: Vec<PhotoId>,
    /// The active photo's EXIF readout.
    exif: Option<repo::PhotoExif>,
    /// Per-field IPTC value across `targets` (same order as `IPTC_FIELDS`).
    iptc: [FieldValues; 4],
    /// The union of keyword paths across `targets` (Keywording chips).
    chips: Vec<String>,
    /// Per-target keyword ids, aligned with `targets` — the Keyword
    /// List's tri-state is computed from these without SQL.
    keyword_ids: Vec<BTreeSet<KeywordId>>,
    /// The catalog's whole keyword list as a display tree.
    keywords: Vec<KeywordNode>,
}

impl PanelCache {
    /// The no-catalog stand-in, so `load_cache` stays total.
    fn empty(key: CacheKey) -> Self {
        Self {
            key,
            targets: Vec::new(),
            exif: None,
            iptc: IPTC_FIELDS.map(|_| FieldValues::Uniform(None)),
            chips: Vec::new(),
            keyword_ids: Vec::new(),
            keywords: Vec::new(),
        }
    }
}

/// A cache is stale exactly when the active photo, the selection, the
/// metadata-dirty counter, or the local keyword-list version moved.
#[derive(Debug, PartialEq)]
struct CacheKey {
    active: Option<PhotoId>,
    selected: Vec<PhotoId>,
    dirty: u64,
    keyword_version: u64,
}

/// One `keywords` row placed in the tree, with its full path
/// (`Places > France`) — the add/remove commands match by full path.
#[derive(Debug)]
struct KeywordNode {
    row: KeywordRow,
    path: String,
    children: Vec<KeywordNode>,
}

/// Paints the whole right panel (plan §7.6), called from
/// `LibraryModule::right_panel` every frame.
pub fn show(ui: &mut egui::Ui, cx: &mut AppCx, data: &LibraryData, state: &mut RightPanelState) {
    // Ctrl+K was pressed in `center()` — focus the Keywording entry.
    if state.focus_keyword_entry {
        state.focus_keyword_entry = false;
        ui.ctx().memory_mut(|m| m.request_focus(keyword_entry_id()));
    }

    if !cx.catalog_open() {
        no_catalog(ui);
        return;
    }

    refresh_cache(cx, data, state);

    metadata_panel(ui, cx, state);
    ui.add_space(12.0);
    keywording_panel(ui, cx, state);
    ui.add_space(12.0);
    keyword_list_panel(ui, cx, state);
    let targets = state
        .cache
        .as_ref()
        .map_or(&[][..], |c| c.targets.as_slice());
    xmp_footer(ui, cx, targets);
}

/// Reloads the cache (and reseeds the IPTC drafts) if the key moved.
fn refresh_cache(cx: &AppCx, data: &LibraryData, state: &mut RightPanelState) {
    let key = CacheKey {
        active: cx.selection.active,
        selected: cx.selection.selected().collect(),
        dirty: data.metadata_dirty,
        keyword_version: state.keyword_list_version,
    };
    if state.cache.as_ref().is_none_or(|c| c.key != key) {
        let cache = load_cache(cx, key);
        // Reseed the drafts from the fresh values so a commit only fires
        // when the user actually edits.
        for (i, values) in cache.iptc.iter().enumerate() {
            let loaded = match values {
                FieldValues::Uniform(value) => value.clone().unwrap_or_default(),
                FieldValues::Mixed => String::new(),
            };
            state.iptc[i] = IptcDraft {
                text: loaded.clone(),
                loaded,
            };
        }
        state.cache = Some(cache);
    }
}

/// One round of every query the panels need. Only called when the cache
/// key changed — this is the sole place that touches SQL.
fn load_cache(cx: &AppCx, key: CacheKey) -> PanelCache {
    let Some(catalog) = &cx.catalog else {
        return PanelCache::empty(key); // unreachable: `show` checks first
    };
    let conn = catalog.connection();
    let targets = shortcuts::targets(cx);

    let exif = key.active.and_then(|id| match repo::photo_exif(conn, id) {
        Ok(exif) => Some(exif),
        Err(e) => {
            tracing::error!(error = %e, "failed to read photo EXIF");
            None
        }
    });

    // IPTC: one query per target, reduced to per-field Uniform/Mixed.
    let mut values: [Vec<Option<String>>; 4] = IPTC_FIELDS.map(|_| Vec::new());
    let mut failed = false;
    for &id in &targets {
        match repo::photo_iptc(conn, id) {
            Ok(i) => {
                values[0].push(i.title);
                values[1].push(i.caption);
                values[2].push(i.creator);
                values[3].push(i.copyright);
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to read photo IPTC");
                failed = true;
            }
        }
    }
    let iptc = if failed {
        IPTC_FIELDS.map(|_| FieldValues::Mixed)
    } else {
        values.map(|vals| match vals.first() {
            None => FieldValues::Uniform(None),
            Some(first) if vals.iter().all(|v| v == first) => FieldValues::Uniform(first.clone()),
            Some(_) => FieldValues::Mixed,
        })
    };

    // Per-target keyword ids (tri-state + chips), also one query each.
    let mut keyword_ids = Vec::with_capacity(targets.len());
    for &id in &targets {
        let ids = match repo::list_keywords_for_photo(conn, id) {
            Ok(ids) => ids,
            Err(e) => {
                tracing::error!(error = %e, "failed to list photo keywords");
                Vec::new()
            }
        };
        keyword_ids.push(ids.into_iter().collect::<BTreeSet<KeywordId>>());
    }

    let rows = match repo::list_keywords(conn) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(error = %e, "failed to list keywords");
            Vec::new()
        }
    };
    let (keywords, paths) = build_keyword_tree(&rows);

    // The union of the targets' keyword paths (Keywording chips).
    let mut chips: BTreeSet<String> = BTreeSet::new();
    for ids in &keyword_ids {
        for id in ids {
            if let Some(path) = paths.get(id) {
                chips.insert(path.clone());
            }
        }
    }

    PanelCache {
        key,
        targets,
        exif,
        iptc,
        chips: chips.into_iter().collect(),
        keyword_ids,
        keywords,
    }
}

/// Nest `list_keywords`' flat rows (by `parent_id`) into a display
/// tree, and give every keyword its full `Places > France` path.
fn build_keyword_tree(rows: &[KeywordRow]) -> (Vec<KeywordNode>, BTreeMap<KeywordId, String>) {
    let by_id: BTreeMap<KeywordId, (Option<KeywordId>, &str)> = rows
        .iter()
        .map(|r| (r.id, (r.parent_id, r.name.as_str())))
        .collect();

    // Walk up the parent chain per keyword. A cycle or a missing parent
    // just truncates the path (both impossible through `upsert_keyword`
    // — the row-count cap only guards against hanging on corrupt data).
    let mut paths: BTreeMap<KeywordId, String> = BTreeMap::new();
    for row in rows {
        let mut segments = vec![row.name.as_str()];
        let mut parent = row.parent_id;
        let mut steps = rows.len();
        while let Some(pid) = parent
            && steps > 0
            && let Some((grandparent, name)) = by_id.get(&pid)
        {
            segments.push(name);
            parent = *grandparent;
            steps -= 1;
        }
        segments.reverse();
        paths.insert(row.id, segments.join(" > "));
    }

    fn build(
        parent: Option<KeywordId>,
        rows: &[KeywordRow],
        by_id: &BTreeMap<KeywordId, (Option<KeywordId>, &str)>,
        paths: &BTreeMap<KeywordId, String>,
    ) -> Vec<KeywordNode> {
        rows.iter()
            // An orphaned parent_id (impossible via the commands, but
            // don't hide rows on corrupt data) is treated as a root.
            .filter(|r| match parent {
                None => r.parent_id.is_none_or(|p| !by_id.contains_key(&p)),
                Some(p) => r.parent_id == Some(p),
            })
            .map(|r| KeywordNode {
                row: r.clone(),
                path: paths.get(&r.id).cloned().unwrap_or_else(|| r.name.clone()),
                children: build(Some(r.id), rows, by_id, paths),
            })
            .collect()
    }

    let keywords = build(None, rows, &by_id, &paths);
    (keywords, paths)
}

// ---------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------

/// Metadata (plan §7.6): the active photo's EXIF as a read-only
/// readout, then the editable IPTC core, one TextEdit per field,
/// targeting whatever a shortcut would target.
fn metadata_panel(ui: &mut egui::Ui, cx: &mut AppCx, state: &mut RightPanelState) {
    let Some(cache) = state.cache.as_ref() else {
        return;
    };

    ui.heading("Metadata");
    ui.add_space(4.0);

    if let Some(exif) = &cache.exif {
        egui::Grid::new("library_metadata_exif")
            .num_columns(2)
            .spacing([10.0, 2.0])
            .show(ui, |ui| {
                exif_row(ui, "File", &exif.filename);
                if !exif.kind.is_empty() {
                    exif_row(ui, "Type", &exif.kind);
                }
                if let Some(size) = exif.file_size {
                    exif_row(ui, "Size", &human_size(size));
                }
                if let (Some(w), Some(h)) = (exif.width, exif.height) {
                    exif_row(ui, "Dimensions", &format!("{w} × {h}"));
                }
                if let Some(time) = &exif.capture_time {
                    exif_row(ui, "Captured", time);
                }
                let camera = match (&exif.camera_make, &exif.camera_model) {
                    (Some(make), Some(model)) => format!("{make} {model}"),
                    (Some(name), None) | (None, Some(name)) => name.clone(),
                    (None, None) => String::new(),
                };
                if !camera.is_empty() {
                    exif_row(ui, "Camera", &camera);
                }
                if let Some(lens) = &exif.lens {
                    exif_row(ui, "Lens", lens);
                }
                if let Some(aperture) = exif.aperture {
                    exif_row(
                        ui,
                        "Aperture",
                        &format!("f/{}", (aperture * 10.0).round() / 10.0),
                    );
                }
                if let Some(shutter) = exif.shutter.filter(|s| *s > 0.0) {
                    exif_row(ui, "Shutter", &shutter_string(shutter));
                }
                if let Some(iso) = exif.iso {
                    exif_row(ui, "ISO", &iso.to_string());
                }
                if let Some(focal) = exif.focal_length {
                    exif_row(ui, "Focal Length", &format!("{focal} mm"));
                }
                if let (Some(lat), Some(lon)) = (exif.gps_lat, exif.gps_lon) {
                    exif_row(ui, "GPS", &format!("{lat:.6}, {lon:.6}"));
                }
            });
    } else {
        ui.weak("No photo active");
    }
    ui.add_space(6.0);

    egui::Grid::new("library_metadata_iptc")
        .num_columns(2)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for (i, field) in IPTC_FIELDS.iter().enumerate() {
                ui.weak(field.label());
                iptc_field(
                    ui,
                    cx,
                    *field,
                    &cache.iptc[i],
                    &mut state.iptc[i],
                    &cache.targets,
                );
                ui.end_row();
            }
        });
}

/// One `label: value` row of the EXIF readout grid.
fn exif_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.weak(label);
    ui.label(value);
    ui.end_row();
}

/// One editable IPTC field: a single-line TextEdit showing the targets'
/// shared value, or a `<mixed>` placeholder when they disagree. Commits
/// on Enter or lost focus — trimmed, with empty clearing the field
/// (plan §7.6).
fn iptc_field(
    ui: &mut egui::Ui,
    cx: &mut AppCx,
    field: IptcField,
    values: &FieldValues,
    draft: &mut IptcDraft,
    targets: &[PhotoId],
) {
    let hint = match values {
        FieldValues::Uniform(value) => value.clone().unwrap_or_default(),
        FieldValues::Mixed => "<mixed>".to_string(),
    };
    let response = ui.add_enabled(
        !targets.is_empty(),
        egui::TextEdit::singleline(&mut draft.text).hint_text(hint),
    );
    let enter = response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    if response.lost_focus() || enter {
        // Only commit a real edit: tabbing through untouched fields —
        // in particular every `<mixed>` one — must not overwrite all
        // targets with the empty draft.
        if draft.text != draft.loaded && !targets.is_empty() {
            let value = draft.text.trim();
            let value = if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            };
            let _ = cx.apply_command(Box::new(SetIptc::new(targets.to_vec(), field, value)));
        }
        if enter {
            response.surrender_focus();
        }
    }
}

fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// The Lightroom-style shutter display: `1/250 s` under a second,
/// `2 s` above.
fn shutter_string(shutter: f64) -> String {
    if shutter < 1.0 {
        format!("1/{} s", (1.0 / shutter).round())
    } else {
        format!("{shutter} s")
    }
}

// ---------------------------------------------------------------------
// Keywording
// ---------------------------------------------------------------------

/// Keywording (plan §7.6): the entry (Enter applies its comma-separated
/// keywords to the targets), autocomplete suggestions for the last
/// typed token, and the union of the targets' keywords as removable
/// chips.
fn keywording_panel(ui: &mut egui::Ui, cx: &mut AppCx, state: &mut RightPanelState) {
    let Some(cache) = state.cache.as_ref() else {
        return;
    };

    ui.heading("Keywording");
    ui.add_space(4.0);

    let response = ui.add_enabled(
        !cache.targets.is_empty(),
        egui::TextEdit::singleline(&mut state.keyword_entry)
            .id(keyword_entry_id())
            .hint_text("Type keywords, comma-separated (Ctrl+K)")
            .desired_width(ui.available_width()),
    );

    // Suggestions follow the entry: recomputed only when it changed.
    if state.suggestion_source != state.keyword_entry {
        state.suggestion_source = state.keyword_entry.clone();
        state.suggestions = suggestions_for(&state.keyword_entry, &cache.keywords);
    }

    if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let names = state
            .keyword_entry
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !names.is_empty() && !cache.targets.is_empty() {
            let _ = cx.apply_command(Box::new(AddKeywords::new(cache.targets.clone(), names)));
        }
        state.keyword_entry.clear();
    }

    if response.has_focus() && !state.suggestions.is_empty() {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            for path in &state.suggestions {
                if ui.small_button(path.as_str()).clicked() {
                    state.keyword_entry = path.clone();
                }
            }
        });
    }

    if !cache.chips.is_empty() {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for chip in &cache.chips {
                ui.horizontal(|ui| {
                    ui.label(chip.as_str());
                    if ui
                        .small_button("×")
                        .on_hover_text(format!("Remove {chip}"))
                        .clicked()
                        && !cache.targets.is_empty()
                    {
                        let _ = cx.apply_command(Box::new(RemoveKeywords::new(
                            cache.targets.clone(),
                            vec![chip.clone()],
                        )));
                    }
                });
            }
        });
    }
}

/// Up to `MAX_SUGGESTIONS` catalog keyword paths containing the last
/// comma-separated token of the entry as a substring (case-insensitive).
fn suggestions_for(entry: &str, keywords: &[KeywordNode]) -> Vec<String> {
    let token = entry.rsplit(',').next().unwrap_or("").trim();
    if token.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    collect_matching(keywords, &token.to_lowercase(), &mut matches);
    matches
}

fn collect_matching(nodes: &[KeywordNode], token: &str, out: &mut Vec<String>) {
    for node in nodes {
        if node.path.to_lowercase().contains(token) {
            out.push(node.path.clone());
        }
        collect_matching(&node.children, token, out);
        if out.len() >= MAX_SUGGESTIONS {
            return;
        }
    }
}

// ---------------------------------------------------------------------
// Keyword List
// ---------------------------------------------------------------------

/// Keyword List (plan §7.6): the catalog's whole keyword tree, one
/// tri-state row per keyword — checked when it's on every target, mixed
/// when on some, unchecked when on none. A click adds (unchecked/mixed)
/// or removes (checked) it on all targets; right-click toggles
/// "Include on export".
fn keyword_list_panel(ui: &mut egui::Ui, cx: &mut AppCx, state: &mut RightPanelState) {
    let Some(cache) = state.cache.as_ref() else {
        return;
    };

    ui.heading("Keyword List");
    ui.add_space(4.0);

    if cache.keywords.is_empty() {
        ui.weak("No keywords yet.");
        return;
    }

    // The "Include on export" toggle is the one mutation that doesn't
    // go through `apply_command` (it isn't a photo change), so it
    // publishes no event — bump the local version to invalidate the
    // cache instead.
    let mut version_bump = false;
    egui::ScrollArea::vertical()
        .id_salt("library_keyword_list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            keyword_nodes(
                ui,
                cx,
                &cache.keywords,
                &cache.targets,
                &cache.keyword_ids,
                &mut version_bump,
                0,
            );
        });
    if version_bump {
        state.keyword_list_version += 1;
    }
}

fn keyword_nodes(
    ui: &mut egui::Ui,
    cx: &mut AppCx,
    nodes: &[KeywordNode],
    targets: &[PhotoId],
    target_keywords: &[BTreeSet<KeywordId>],
    version_bump: &mut bool,
    depth: usize,
) {
    for node in nodes {
        keyword_row(ui, cx, node, targets, target_keywords, version_bump, depth);
        if !node.children.is_empty() {
            keyword_nodes(
                ui,
                cx,
                &node.children,
                targets,
                target_keywords,
                version_bump,
                depth + 1,
            );
        }
    }
}

fn keyword_row(
    ui: &mut egui::Ui,
    cx: &mut AppCx,
    node: &KeywordNode,
    targets: &[PhotoId],
    target_keywords: &[BTreeSet<KeywordId>],
    version_bump: &mut bool,
    depth: usize,
) {
    let count = target_keywords
        .iter()
        .filter(|set| set.contains(&node.row.id))
        .count();
    let tri = if targets.is_empty() || count == 0 {
        Tri::Unchecked
    } else if count == targets.len() {
        Tri::Checked
    } else {
        Tri::Mixed
    };

    let mut activate = false;
    let row = ui.horizontal(|ui| {
        // Same flat-indent convention as the Folders panel (§7.2).
        ui.add_space(depth as f32 * 12.0);
        activate |= tri_state_box(ui, tri).clicked();
        activate |= ui
            .add(egui::Label::new(node.row.name.as_str()).sense(egui::Sense::click()))
            .clicked();
        ui.weak(format!("({})", node.row.photo_count));
        if node.row.include_on_export {
            let _ = ui.small("⇩").on_hover_text("Included on export");
        }
    });

    // Click semantics: checked -> remove from all targets, otherwise add.
    if activate && !targets.is_empty() {
        if tri == Tri::Checked {
            let _ = cx.apply_command(Box::new(RemoveKeywords::new(
                targets.to_vec(),
                vec![node.path.clone()],
            )));
        } else {
            let _ = cx.apply_command(Box::new(AddKeywords::new(
                targets.to_vec(),
                vec![node.path.clone()],
            )));
        }
    }

    row.response.context_menu(|ui| {
        let label = if node.row.include_on_export {
            "Don't Include on Export"
        } else {
            "Include on Export"
        };
        if ui.button(label).clicked() {
            let Some(catalog) = &cx.catalog else { return };
            let include = !node.row.include_on_export;
            if let Err(e) =
                repo::set_keyword_include_on_export(catalog.connection(), node.row.id, include)
            {
                tracing::error!(error = %e, "failed to set include_on_export");
            } else {
                *version_bump = true;
            }
            ui.close();
        }
    });
}

/// A hand-painted tri-state checkbox (egui's is two-state): a check
/// when on every target, a dash when on some, empty when on none.
fn tri_state_box(ui: &mut egui::Ui, tri: Tri) -> egui::Response {
    let size = egui::Vec2::splat(ui.spacing().interact_size.y * 0.85);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let corner = egui::CornerRadius::same(3);
    ui.painter().rect_filled(rect, corner, visuals.bg_fill);
    ui.painter()
        .rect_stroke(rect, corner, visuals.fg_stroke, egui::StrokeKind::Inside);
    let glyph = match tri {
        Tri::Unchecked => None,
        Tri::Mixed => Some("–"),
        Tri::Checked => Some("✓"),
    };
    if let Some(glyph) = glyph {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::proportional(rect.height() * 0.9),
            ui.style().visuals.strong_text_color(),
        );
    }
    response
}

// ---------------------------------------------------------------------
// XMP footer
// ---------------------------------------------------------------------

/// XMP footer (plan §7.6, D5): manual "Save to XMP" for the targets as
/// a background job, plus the persisted auto-write preference (checked
/// in `AppCx::apply_command`).
fn xmp_footer(ui: &mut egui::Ui, cx: &mut AppCx, targets: &[PhotoId]) {
    ui.separator();

    let can_save = cx.settings.last_catalog.is_some() && !targets.is_empty();
    let save = ui
        .add_enabled(can_save, egui::Button::new("Save to XMP"))
        .on_disabled_hover_text(if cx.settings.last_catalog.is_none() {
            "No catalog open"
        } else {
            "Nothing selected"
        });
    if save.clicked()
        && let Some(path) = cx.settings.last_catalog.clone()
        && !targets.is_empty()
    {
        cx.jobs.submit(SaveXmpJob::new(path, targets.to_vec()));
    }

    let mut auto = cx.settings.xmp_auto_write == XmpAutoWrite::On;
    if ui
        .checkbox(&mut auto, "Automatically write changes to XMP")
        .changed()
    {
        cx.settings.xmp_auto_write = if auto {
            XmpAutoWrite::On
        } else {
            XmpAutoWrite::Off
        };
        if let Err(e) = cx.settings.save() {
            tracing::error!(error = %e, "failed to save settings");
        }
    }
}

/// The panel headers with a note, when no catalog is open — nothing to
/// query, so don't even paint the controls.
fn no_catalog(ui: &mut egui::Ui) {
    for heading in ["Metadata", "Keywording", "Keyword List"] {
        ui.heading(heading);
        ui.add_space(4.0);
        ui.weak("No catalog open");
        ui.add_space(12.0);
    }
}
