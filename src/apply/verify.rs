//! The check before a swap: whether a remuxed file is its original with the fixed tracks replaced
//! and nothing else changed. Every difference found is listed, so a failure says what differs.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::audio;
use crate::blocks::{self, Block, Timed};
use crate::ebml::{self, Reader, Result};
use crate::iso639;
use crate::mkv::{self, Flags, Layout, Track};
use crate::subs::{self, Kind};
use crate::text::{timestamp, timestamp_ms};

/// An open file.
#[derive(Clone, Copy)]
pub struct File<'a> {
    pub r: &'a Reader,
    pub layout: &'a Layout,
}

/// What differs between `original`, with the blocks of `fixed` (by track ID) in place of its
/// own, and `remuxed`; empty when nothing does. `read` holds the original's text tracks as already
/// read, by track ID; any other is read here. `full` adds every frame of every other track,
/// compared by hash: it reads both files end to end.
pub fn compare(
    original: File,
    remuxed: File,
    fixed: &BTreeMap<usize, &[Block]>,
    read: &BTreeMap<usize, &[Block]>,
    languages: &Languages,
    full: bool,
) -> Result<Vec<String>> {
    let (a, b) = (original.layout, remuxed.layout);
    let mut diffs = Vec::new();
    // mkvmerge always writes one: a new UID where the original had none (ffmpeg's bitexact
    // output) is no difference, since nothing could have referred to it.
    if a.segment_uid.is_some() && a.segment_uid != b.segment_uid {
        diffs.push("segment UID differs".into());
    }
    if a.title != b.title {
        diffs.push(format!("title differs: {:?}, was {:?}", b.title, a.title));
    }
    let chapters_of = |f: File| {
        f.layout
            .chapters_at
            .map_or(Ok(None), |at| chapters(f.r, at))
    };
    let renumbered = renumbers_chapters(a.writing_app.as_deref());
    if !same_chapters(chapters_of(original)?, chapters_of(remuxed)?, renumbered) {
        diffs.push("chapters differ".into());
    }
    if attachments(original)? != attachments(remuxed)? {
        diffs.push("attachments differ".into());
    }
    if a.tracks.len() != b.tracks.len() {
        diffs.push(format!("{} tracks, was {}", b.tracks.len(), a.tracks.len()));
        return Ok(diffs);
    }
    for (t, u) in a.tracks.iter().zip(&b.tracks) {
        header(
            t,
            languages.get(&t.uid),
            u,
            fixed.contains_key(&t.id),
            &mut diffs,
        )?;
    }
    if !diffs.is_empty() {
        return Ok(diffs);
    }
    subtitles(original, remuxed, fixed, read, &mut diffs)?;
    others(original, remuxed, full, &mut diffs)?;
    audio_times(original, remuxed, &mut diffs)?;
    Ok(diffs)
}

/// The Chapters element at `at`, as compared. Chapters that do not parse are an error, not "no
/// chapters": mkvmerge may drop what it cannot read.
fn chapters(r: &Reader, at: u64) -> Result<Option<Vec<(u32, Value)>>> {
    values(&r.data(&r.header_at(at)?)?, CHAPTER_MASTERS)
        .map(Some)
        .ok_or_else(|| ebml::Error::Unsupported("chapters that do not parse".into()))
}

/// Whether mkvmerge gives new UIDs to the editions and chapters of a file `writing_app` wrote (and
/// to the tags that target them): it keeps only those of a file whose WritingApp starts
/// `mkvmerge` or `no_variable_data`, any case, since HandBrake and others number them from 1. A
/// file that names no WritingApp keeps its own.
fn renumbers_chapters(writing_app: Option<&str>) -> bool {
    writing_app.is_some_and(|app| {
        let app = app.trim().to_ascii_lowercase();
        !app.starts_with("mkvmerge") && !app.starts_with("no_variable_data")
    })
}

/// Whether the remux's chapters (`now`) are the original's (`was`), as mkvmerge writes them: with
/// new edition and chapter UIDs if it `renumbered` them, an EditionUID for an edition that had none
/// (it gives each one), and each display's languages as players read them (see
/// [`display_languages`]). Nothing could refer to a UID that was not there.
fn same_chapters(
    was: Option<Vec<(u32, Value)>>,
    now: Option<Vec<(u32, Value)>>,
    renumbered: bool,
) -> bool {
    match (was, now) {
        (Some(mut was), Some(mut now)) => {
            without_mkvmerges_own(&mut was, &mut now, renumbered);
            was == now
        }
        (was, now) => was == now,
    }
}

/// The children of one master of the original's chapters (`was`) and the remux's (`now`), with
/// what [`same_chapters`] passes left out, and each master inside likewise with the one of its ID
/// in the same place among the original's.
fn without_mkvmerges_own(
    was: &mut Vec<(u32, Value)>,
    now: &mut Vec<(u32, Value)>,
    renumbered: bool,
) {
    if renumbered {
        was.retain(|(id, _)| *id != EDITION_UID && *id != CHAPTER_UID);
        now.retain(|(id, _)| *id != EDITION_UID && *id != CHAPTER_UID);
    }
    if !was.iter().any(|(id, _)| *id == EDITION_UID) {
        now.retain(|(id, _)| *id != EDITION_UID);
    }
    let mut seen: HashMap<u32, usize> = HashMap::new();
    for (id, value) in now.iter_mut() {
        let Value::Master(ours) = value else {
            continue;
        };
        let nth = seen.entry(*id).or_default();
        let theirs = was.iter_mut().filter(|(w, _)| w == id).nth(*nth);
        *nth += 1;
        if let Some((_, Value::Master(theirs))) = theirs {
            if *id == CHAPTER_DISPLAY {
                display_languages(theirs);
                display_languages(ours);
            }
            without_mkvmerges_own(theirs, ours, renumbered);
        }
    }
}

/// A ChapterDisplay's children with its languages as players read them in place of its ChapLanguage
/// and ChapLanguageBCP47 elements: its BCP 47 tags if it has any, else its ChapLanguages as BCP 47
/// (`fr` for `fre` and `fra`, `haw` as it is), else Matroska's `en`; lowercase, as a set. mkvmerge
/// writes ChapLanguage lowercase and bibliographic, `eng` where there was none, and adds the tag
/// from it (checked, mkvmerge v103.0).
fn display_languages(children: &mut Vec<(u32, Value)>) {
    let of = |id: u32| -> Vec<String> {
        children
            .iter()
            .filter(|(i, _)| *i == id)
            .map(|(_, v)| text_of(v).to_ascii_lowercase())
            .collect()
    };
    let mut languages = of(CHAP_LANGUAGE_BCP47);
    if languages.is_empty() {
        languages = of(CHAP_LANGUAGE)
            .into_iter()
            .map(|code| iso639::alpha_2(&code).map_or(code, str::to_string))
            .collect();
    }
    if languages.is_empty() {
        languages.push("en".into());
    }
    languages.sort();
    languages.dedup();
    children.retain(|(id, _)| *id != CHAP_LANGUAGE && *id != CHAP_LANGUAGE_BCP47);
    children.push((
        CHAP_LANGUAGE_BCP47,
        Value::Raw(languages.join(" ").into_bytes()),
    ));
}

/// A string element's text as [`value`] holds it (up to 8 bytes as an integer), read as Matroska
/// reads a string: up to its first NUL, here trimmed. Empty for any other value.
fn text_of(v: &Value) -> String {
    let text = match v {
        Value::Uint(u) => {
            let bytes = u.to_be_bytes();
            ebml::string(&bytes[bytes.iter().take_while(|&&b| b == 0).count()..])
        }
        Value::Raw(bytes) => ebml::string(bytes),
        Value::Float(_) | Value::Master(_) => String::new(),
    };
    text.trim().to_string()
}

/// One attached file as its elements, in order: (ID, data).
type Attached = Vec<(u32, Vec<u8>)>;

/// The attached files, each as its elements in order, the MIME type left out (mkvmerge 102
/// rewrites `application/x-truetype-font` as `font/ttf`) and the data as its length and hash.
fn attachments(f: File) -> Result<Vec<Attached>> {
    f.layout
        .attachments_at
        .map_or(Ok(Vec::new()), |at| attached_files(f.r, at))
}

/// The attached files of the Attachments element at `at`, one at a time: together they can
/// outgrow what one read may hold. Its CRC-32 or Void (ffmpeg writes a CRC-32 into each
/// top-level element) is no attached file.
fn attached_files(r: &Reader, at: u64) -> Result<Vec<Attached>> {
    let h = r.header_at(at)?;
    let end = h.end().unwrap_or(h.data_start);
    let mut out = Vec::new();
    let mut pos = h.data_start;
    while pos < end {
        let file = r.header_at(pos)?;
        if file.id == ATTACHED_FILE {
            out.push(attached(&r.data(&file)?, file.data_start)?);
        }
        pos = file.end().unwrap_or(end);
    }
    Ok(out)
}

/// One AttachedFile's data (at `base`) as compared: its elements but the MIME type, the file's
/// bytes as their length and hash, its UID by value (ffmpeg writes one in 8 bytes, zeros ahead,
/// that mkvmerge writes in fewer).
fn attached(data: &[u8], base: u64) -> Result<Attached> {
    let mut elements = Vec::new();
    for (id, _, v) in ebml::children(data, base)? {
        match id {
            FILE_MIME_TYPE | mkv::CRC32 | mkv::VOID => {}
            FILE_DATA => elements.push((id, digest_of(v))),
            FILE_UID if v.len() <= 8 => elements.push((id, ebml::uint(v).to_be_bytes().to_vec())),
            _ => elements.push((id, v.to_vec())),
        }
    }
    Ok(elements)
}

const ATTACHED_FILE: u32 = 0x61A7;
const FILE_MIME_TYPE: u32 = 0x4660;
const FILE_DATA: u32 = 0x465C;
const FILE_UID: u32 = 0x46AE;

fn digest_of(data: &[u8]) -> Vec<u8> {
    let mut h = DefaultHasher::new();
    data.hash(&mut h);
    [data.len() as u64, h.finish()]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect()
}

/// A track's language as mkvmerge reads it from the original (`mkvmerge -J`), and so writes it in a
/// remux: its BCP 47 tag in canonical form (`iw` as `he`), or for a track with no tag its ISO
/// 639-2 code, lowercase and bibliographic (`fra` and `fr` as `fre`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Language {
    Bcp47(String),
    Iso639_2(String),
}

/// The original's tracks' languages as mkvmerge reads them, by track UID.
pub type Languages = BTreeMap<u64, Language>;

/// A track's header against its original's. `fixed`: the track was replaced, so its UID is new
/// and its CodecPrivate rebuilt from the `.ass` header.
fn header(
    t: &Track,
    read_as: Option<&Language>,
    u: &Track,
    fixed: bool,
    diffs: &mut Vec<String>,
) -> Result<()> {
    let mut differs = |what: &str| diffs.push(format!("track {}: {what} differs", t.id));
    if t.kind != u.kind || t.codec != u.codec {
        differs("codec");
    }
    // Names read trimmed, and empty or blank as none (see the parser), as mkvmerge writes them.
    if t.name != u.name {
        differs("name");
    }
    // As mkvmerge reads the original's language (`read_as`), so it writes it. Beside a BCP 47 tag
    // Matroska ignores Language, which mkvmerge writes from the tag; without one, mkvmerge adds
    // the tag, which is no difference. A track mkvmerge did not read is compared as the file has
    // it, a tag (case aside) before Language.
    let tag_is = |tag: &str| {
        (u.language_bcp47.as_deref()).is_some_and(|theirs| theirs.eq_ignore_ascii_case(tag))
    };
    let same_language = match (read_as, &t.language_bcp47) {
        (Some(Language::Bcp47(tag)), _) | (None, Some(tag)) => tag_is(tag),
        (Some(Language::Iso639_2(code)), _) => u.language.eq_ignore_ascii_case(code),
        (None, None) => t.language == u.language,
    };
    if !same_language {
        differs("language");
    }
    if t.default != u.default || t.forced != u.forced || flags(&t.flags) != flags(&u.flags) {
        differs("flags");
    }
    // mkvmerge leaves a text track's DefaultDuration out of any remux and writes each block's
    // duration instead, which the events are compared with. It gives an audio track that had none
    // (ffmpeg 7 wrote none) its codec's frame length, FLAC's 96 ms, AAC's 23.2: the frames are
    // compared below, and their times, a frame laced behind its block's first timed by this.
    if t.default_duration != u.default_duration
        && !(t.is_text_subtitle() && u.default_duration.is_none())
        && !(t.kind == mkv::TRACK_TYPE_AUDIO && t.default_duration.is_none())
    {
        differs("default duration");
    }
    if t.encodings != u.encodings {
        differs("compression");
    }
    let private = |track: &Track| {
        blocks::decode(
            track.codec_private.clone(),
            &track.encodings,
            mkv::SCOPE_CODEC_PRIVATE,
        )
    };
    let (p, q) = (private(t)?, private(u)?);
    if if fixed {
        script(&p) != script(&q)
    } else {
        p != q
    } {
        differs("codec private data");
    }
    if !same_properties(t.video.as_deref(), u.video.as_deref())
        || !same_properties(t.audio.as_deref(), u.audio.as_deref())
    {
        differs("video or audio properties");
    }
    Ok(())
}

/// The flags with Matroska's defaults filled in: a file that leaves FlagEnabled out says the same
/// as one that sets it to 1.
fn flags(f: &Flags) -> [bool; 6] {
    [
        f.enabled.unwrap_or(true),
        f.hearing_impaired.unwrap_or(false),
        f.visual_impaired.unwrap_or(false),
        f.text_descriptions.unwrap_or(false),
        f.original.unwrap_or(false),
        f.commentary.unwrap_or(false),
    ]
}

/// An ASS header as mkvextract writes it and mkvmerge reads it back: its lines but the blank ones,
/// with LF line ends, and nothing blank at its end. A section that followed `[Events]` (Aegisub's
/// extradata, in K-On!! E09) is written after the events, and mkvmerge reads the blank line before
/// it into the header. Bytes, not text: a rewrite that changed bytes that are not UTF-8 must show.
fn script(private: &[u8]) -> Vec<u8> {
    let blank = |b: &u8| matches!(b, b' ' | b'\t' | b'\r');
    let lines: Vec<&[u8]> = subs::c_bytes(private)
        .split(|&b| b == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
        .filter(|l| !l.iter().all(blank))
        .collect();
    let mut out = lines.join(&b'\n');
    while out.last().is_some_and(blank) {
        out.pop();
    }
    out
}

/// Video and Audio elements whose children are floats; any other child of 8 bytes or fewer is an
/// unsigned integer. mkvmerge may write an integer wider than the original did (DisplayWidth in
/// 4 bytes where it had 2), so they are compared by value.
const FLOATS: &[u32] = &[
    0xB5,
    0x78B5,
    0x0023_83E3,
    0x55D1,
    0x55D2,
    0x55D3,
    0x55D4,
    0x55D5,
    0x55D6,
    0x55D7,
    0x55D8,
    0x55D9,
    0x55DA,
    0x7673,
    0x7674,
    0x7675,
];
/// Masters inside Video: Colour, MasteringMetadata, Projection.
const VIDEO_MASTERS: &[u32] = &[0x55B0, 0x55D0, 0x7670];
const EDITION_UID: u32 = 0x45BC;
const CHAPTER_UID: u32 = 0x73C4;
const CHAPTER_DISPLAY: u32 = 0x80;
const CHAP_LANGUAGE: u32 = 0x437C;
const CHAP_LANGUAGE_BCP47: u32 = 0x437D;
/// Masters inside Chapters: EditionEntry, ChapterAtom, ChapterTrack, ChapterDisplay, ChapProcess,
/// ChapProcessCommand, EditionDisplay.
const CHAPTER_MASTERS: &[u32] = &[0x45B9, 0xB6, 0x8F, 0x80, 0x6944, 0x6911, 0x4520];

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Value {
    Uint(u64),
    Float(u64),
    Raw(Vec<u8>),
    Master(Vec<(u32, Value)>),
}

/// Whether a track's Video or Audio element in the remux (`now`) says what the original's (`was`)
/// does: by value, each child Matroska would take as said when left out left out (see
/// [`default_of`]); as bytes, if either does not parse. And mkvmerge 103 keeps no FlagInterlaced
/// from a Matroska file (checked on progressive H.264, HEVC, MPEG-2 and VP9, each of which says in
/// its stream whether it is interlaced): a 2, progressive, left out passes. A 1 left out changes
/// what a player that goes by the flag does, and does not.
fn same_properties(was: Option<&[u8]>, now: Option<&[u8]>) -> bool {
    match (was, now) {
        (None, None) => true,
        (Some(a), Some(b)) => match (values(a, VIDEO_MASTERS), values(b, VIDEO_MASTERS)) {
            (Some(was), Some(now)) => {
                let (mut was, now) = (explicit(was), explicit(now));
                if !now.iter().any(|(id, _)| *id == FLAG_INTERLACED) {
                    was.retain(|(id, v)| !(*id == FLAG_INTERLACED && *v == Value::Uint(2)));
                }
                was == now
            }
            _ => a == b,
        },
        _ => false,
    }
}

/// A Video or Audio element's children without those that say what Matroska takes them to be
/// when left out: mkvmerge leaves out a Channels of 1 and a SamplingFrequency of 8000 that ffmpeg
/// writes, and writes a DisplayWidth and DisplayHeight of the pixel size that ffmpeg leaves out.
fn explicit(children: Vec<(u32, Value)>) -> Vec<(u32, Value)> {
    let defaults: Vec<Option<Value>> = children
        .iter()
        .map(|(id, _)| default_of(*id, &children))
        .collect();
    children
        .into_iter()
        .zip(defaults)
        .filter(|((_, v), default)| default.as_ref() != Some(v))
        .map(|(child, _)| child)
        .collect()
}

/// What Matroska takes the Video or Audio child `id` to be when left out, beside its `siblings`,
/// where it says.
fn default_of(id: u32, siblings: &[(u32, Value)]) -> Option<Value> {
    let get = |id: u32| siblings.iter().find(|(i, _)| *i == id).map(|(_, v)| v);
    let uint = |id: u32, default: u64| match get(id) {
        Some(Value::Uint(v)) => Some(*v),
        Some(_) => None,
        None => Some(default),
    };
    // In pixels, the width or height less its crops.
    let shown = |size: u32, crops: [u32; 2]| {
        if uint(DISPLAY_UNIT, 0)? != 0 {
            return None;
        }
        let size = uint(size, 0).filter(|&s| s > 0)?;
        let crop = uint(crops[0], 0)?.checked_add(uint(crops[1], 0)?)?;
        size.checked_sub(crop).map(Value::Uint)
    };
    match id {
        FLAG_INTERLACED | STEREO_MODE | ALPHA_MODE | PIXEL_CROP_BOTTOM | PIXEL_CROP_TOP
        | PIXEL_CROP_LEFT | PIXEL_CROP_RIGHT | DISPLAY_UNIT | ASPECT_RATIO_TYPE | EMPHASIS => {
            Some(Value::Uint(0))
        }
        FIELD_ORDER => Some(Value::Uint(2)),
        CHANNELS => Some(Value::Uint(1)),
        SAMPLING_FREQUENCY => Some(Value::Float(8000f64.to_bits())),
        OUTPUT_SAMPLING_FREQUENCY => match get(SAMPLING_FREQUENCY) {
            Some(Value::Float(rate)) => Some(Value::Float(*rate)),
            Some(_) => None,
            None => Some(Value::Float(8000f64.to_bits())),
        },
        DISPLAY_WIDTH => shown(PIXEL_WIDTH, [PIXEL_CROP_LEFT, PIXEL_CROP_RIGHT]),
        DISPLAY_HEIGHT => shown(PIXEL_HEIGHT, [PIXEL_CROP_TOP, PIXEL_CROP_BOTTOM]),
        _ => None,
    }
}

const FLAG_INTERLACED: u32 = 0x9A;
const FIELD_ORDER: u32 = 0x9D;
const STEREO_MODE: u32 = 0x53B8;
const ALPHA_MODE: u32 = 0x53C0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const PIXEL_CROP_BOTTOM: u32 = 0x54AA;
const PIXEL_CROP_TOP: u32 = 0x54BB;
const PIXEL_CROP_LEFT: u32 = 0x54CC;
const PIXEL_CROP_RIGHT: u32 = 0x54DD;
const DISPLAY_WIDTH: u32 = 0x54B0;
const DISPLAY_HEIGHT: u32 = 0x54BA;
const DISPLAY_UNIT: u32 = 0x54B2;
const ASPECT_RATIO_TYPE: u32 = 0x54B3;
const SAMPLING_FREQUENCY: u32 = 0xB5;
const OUTPUT_SAMPLING_FREQUENCY: u32 = 0x78B5;
const CHANNELS: u32 = 0x9F;
const EMPHASIS: u32 = 0x52F1;

/// A master's children as values, sorted, its CRC-32 and Void left out at every level, `masters`
/// parsed as masters in turn; `None` when they do not parse.
fn values(data: &[u8], masters: &[u32]) -> Option<Vec<(u32, Value)>> {
    let mut out = Vec::new();
    for (id, _, v) in ebml::children(data, 0).ok()? {
        if id == mkv::CRC32 || id == mkv::VOID {
            continue;
        }
        let value = if masters.contains(&id) {
            Value::Master(values(v, masters)?)
        } else {
            value(id, v)
        };
        out.push((id, value));
    }
    // By ID only, and stably: siblings of one ID (chapter atoms, editions) keep their order,
    // which is playback order.
    out.sort_by_key(|(id, _)| *id);
    Some(out)
}

/// A leaf as a value: a float by its value, anything else of 8 bytes or fewer as an unsigned
/// integer (mkvmerge may write one wider than the original did), longer data as it is.
fn value(id: u32, v: &[u8]) -> Value {
    if FLOATS.contains(&id)
        && let Some(f) = ebml::float(v)
    {
        return Value::Float(f.to_bits());
    }
    if v.len() <= 8 {
        Value::Uint(ebml::uint(v))
    } else {
        Value::Raw(v.to_vec())
    }
}

/// Every text subtitle track, event by event: start, duration and text, against the original's
/// (or, for a fixed track, its patched blocks).
fn subtitles(
    original: File,
    remuxed: File,
    fixed: &BTreeMap<usize, &[Block]>,
    read: &BTreeMap<usize, &[Block]>,
    diffs: &mut Vec<String>,
) -> Result<()> {
    let text: Vec<(&Track, &Track)> = original
        .layout
        .tracks
        .iter()
        .zip(&remuxed.layout.tracks)
        .filter(|(t, _)| t.is_text_subtitle())
        .collect();
    let reread = |f: File, tracks: Vec<&Track>| {
        let numbers: Vec<u64> = tracks.iter().map(|t| t.number).collect();
        blocks::read_tracks(f.r, f.layout, &numbers, must_walk(f.layout, &tracks))
    };
    // What the caller has read already is not read again, nor are the fixed tracks: their patched
    // blocks are what to expect.
    let unread: Vec<&Track> = text
        .iter()
        .map(|(t, _)| *t)
        .filter(|t| !fixed.contains_key(&t.id) && !read.contains_key(&t.id))
        .collect();
    let before = if unread.is_empty() {
        Vec::new()
    } else {
        reread(original, unread)?
    };
    let after = reread(remuxed, text.iter().map(|(_, u)| *u).collect())?;
    let scale = remuxed.layout.timestamp_scale;
    for ((t, u), now) in text.iter().zip(&after) {
        let kind = Kind::of(t).expect("a text subtitle track");
        let expected: &[Block] = match fixed.get(&t.id).or(read.get(&t.id)) {
            Some(blocks) => blocks,
            None => {
                &before
                    .iter()
                    .find(|b| b.track == t.number)
                    .expect("every other text track was read here")
                    .blocks
            }
        };
        // A block with no duration of its own, on either side, as the remux has or would have
        // written its track's DefaultDuration in it.
        let want = events(
            kind,
            expected,
            original.layout.timestamp_scale,
            as_remuxed(t.default_duration, scale),
            fixed.contains_key(&t.id),
        );
        let got = events(
            kind,
            &now.blocks,
            scale,
            as_remuxed(u.default_duration, scale),
            fixed.contains_key(&t.id),
        );
        if want.len() != got.len() {
            diffs.push(format!(
                "track {}: {} events, was {}",
                t.id,
                got.len(),
                want.len()
            ));
        } else if let Some(i) = (0..want.len()).find(|&i| want[i] != got[i]) {
            let at = timestamp(i64::try_from(want[i].0 / 1_000_000).unwrap_or(0));
            diffs.push(format!("track {}: the event at {at} differs", t.id));
        }
    }
    Ok(())
}

/// Whether reading `tracks` of a file must walk its Clusters: its Cues are trusted only where its
/// statistics say how many blocks they should lead to, and a track with Cues for only some of its
/// blocks would otherwise lose the rest unseen (mkvmerge writes the statistics; ffmpeg writes none,
/// and those it keeps from an earlier mux do not count).
pub fn must_walk(layout: &Layout, tracks: &[&Track]) -> bool {
    tracks
        .iter()
        .any(|t| !layout.frame_counts.contains_key(&t.uid))
}

/// A text track's DefaultDuration, in ns, as a remux at `scale` writes it in a block with no
/// duration of its own: whole `scale` units, rounded to the nearest (mkvmerge 74, 82, 102 and 103
/// make 42 ms of 41.708333; 102 makes 42 of 41.5 and 41 of 41.499999).
pub fn as_remuxed(default_duration: Option<u64>, scale: u64) -> Option<i128> {
    let scale = i128::from(scale.max(1));
    default_duration.map(|d| (i128::from(d) + scale / 2) / scale * scale)
}

/// Each block as (start ns, duration ns, bytes up to its first NUL): a block with no duration of
/// its own has `default` (ns); ASS's ReadOrder as its rank in the track (mkvmerge numbers events
/// afresh, in file order), and for a rewritten SRT track no CRs (mkvmerge puts CRLF between an
/// event's rows whatever its source had; ffmpeg puts LF). Bytes, not text: a rewrite that changed
/// bytes that are not UTF-8 must show.
fn events(
    kind: Kind,
    blocks: &[Block],
    scale: u64,
    default: Option<i128>,
    rewritten: bool,
) -> Vec<(i128, Option<i128>, Vec<u8>)> {
    let mut texts: Vec<Vec<u8>> = blocks
        .iter()
        .map(|b| subs::c_bytes(&b.payload).to_vec())
        .collect();
    if kind != Kind::Srt {
        let mut orders: Vec<(i64, usize)> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| (subs::read_order(t).unwrap_or(i64::MAX), i))
            .collect();
        orders.sort_unstable();
        for (rank, &(_, i)) in orders.iter().enumerate() {
            let rest = texts[i]
                .iter()
                .position(|&b| b == b',')
                .map_or(Vec::new(), |c| texts[i][c..].to_vec());
            texts[i] = [rank.to_string().into_bytes(), rest].concat();
        }
    } else if rewritten {
        for t in &mut texts {
            t.retain(|&b| b != b'\r');
        }
    }
    blocks
        .iter()
        .zip(texts)
        .map(|(b, text)| {
            let (start, duration) = expected(b, scale, default);
            (start, duration, text)
        })
        .collect()
}

/// A block's start and duration in ns, as the check compares them: a block with no duration of
/// its own has `default` (its track's DefaultDuration as a remux writes it: [`as_remuxed`]).
pub fn expected(b: &Block, scale: u64, default: Option<i128>) -> (i128, Option<i128>) {
    let scale = i128::from(scale);
    let duration = b.duration.map(|d| i128::from(d) * scale).or(default);
    (i128::from(b.timestamp) * scale, duration)
}

/// Every other track (video, audio, image subtitles): its frame count and byte total, from both
/// files' statistics where both have them; else, and always when `full`, every frame by hash.
fn others(original: File, remuxed: File, full: bool, diffs: &mut Vec<String>) -> Result<()> {
    let (a, b) = (original.layout, remuxed.layout);
    let mut walk = Vec::new();
    for (t, u) in a.tracks.iter().zip(&b.tracks) {
        if t.is_text_subtitle() {
            continue;
        }
        let stats =
            |l: &Layout, uid: u64| Some((*l.frame_counts.get(&uid)?, *l.byte_counts.get(&uid)?));
        match (stats(a, t.uid), stats(b, u.uid)) {
            (Some(x), Some(y)) if !full => {
                if x != y {
                    diffs.push(format!(
                        "track {}: {} frames of {} bytes, was {} of {}",
                        t.id, y.0, y.1, x.0, x.1
                    ));
                }
            }
            _ => walk.push((t, u)),
        }
    }
    if walk.is_empty() {
        return Ok(());
    }
    let (x, y) = (digests(original)?, digests(remuxed)?);
    for (t, u) in walk {
        if x.get(&t.number) != y.get(&u.number) {
            diffs.push(format!("track {}: frames differ", t.id));
        }
    }
    Ok(())
}

/// How long each span [`audio_times`] reads is.
const SPAN_NS: u64 = 5_000_000_000;
/// How far an audio frame's time may move in a remux: mkvmerge stores times to the millisecond,
/// and a plain remux moves a few audio frames by 1-2 ms.
const MOVED_NS: i128 = 2_000_000;
/// How far around its old time a frame is looked for in the remux.
const SEARCH_NS: i128 = 1_000_000_000;

/// Whether every audio track's frames kept their times, frame by frame, in six spans of the file:
/// frame counts and bytes say nothing of times, and a remux can move a track. mkvmerge 102 stamped
/// FMAB's TrueHD by frame count over a hole at its head, 67 ms early there. Only frames whose bytes
/// occur once in their span count: digital silence repeats one frame, which would match anywhere.
/// The first frame out of place is named, once per track.
fn audio_times(original: File, remuxed: File, diffs: &mut Vec<String>) -> Result<()> {
    let (a, b) = (original.layout, remuxed.layout);
    let audio: Vec<(&Track, &Track)> = a
        .tracks
        .iter()
        .zip(&b.tracks)
        .filter(|(t, _)| t.kind == mkv::TRACK_TYPE_AUDIO)
        .collect();
    if audio.is_empty() {
        return Ok(());
    }
    let was_numbers: Vec<u64> = audio.iter().map(|(t, _)| t.number).collect();
    let now_numbers: Vec<u64> = audio.iter().map(|(_, u)| u.number).collect();
    let at = |ns: i128| timestamp_ms(i64::try_from(ns / 1_000_000).unwrap_or(i64::MAX));
    let mut moved = HashSet::new();
    for from in spans(a.duration_ns) {
        let to = from + i128::from(SPAN_NS);
        let was = blocks::frames_between(original.r, a, from, to, &was_numbers)?;
        let now =
            blocks::frames_between(remuxed.r, b, from - SEARCH_NS, to + SEARCH_NS, &now_numbers)?;
        for (t, u) in &audio {
            let Some(frames) = was.get(&t.number).filter(|_| !moved.contains(&t.id)) else {
                continue;
            };
            let there = now.get(&u.number).map_or(&[][..], Vec::as_slice);
            if let Some((old, new)) = first_moved(frames, there) {
                diffs.push(match new {
                    Some(new) => format!(
                        "track {}: audio frame at {} is at {} in the remux",
                        t.id,
                        at(old),
                        at(new)
                    ),
                    None => format!(
                        "track {}: audio frame at {} is missing from the remux (none within a second)",
                        t.id,
                        at(old)
                    ),
                });
                moved.insert(t.id);
            }
        }
    }
    Ok(())
}

/// The first frame of `was` that `now` has out of place: its time, and its time in `now` (`None`
/// where `now` has no copy of it). Only frames whose bytes occur once in `was` and that have a time
/// count. A copy in `now` with no time to read (laced behind its block's first, with no duration)
/// says nothing, so it passes: a track laced that way (mkvmerge laces Vorbis eight to a block,
/// with no durations) is checked only at the frames that start a block in both files.
fn first_moved(was: &[Timed], now: &[Timed]) -> Option<(i128, Option<i128>)> {
    let mut copies: HashMap<u64, usize> = HashMap::new();
    for f in was {
        *copies.entry(f.hash).or_default() += 1;
    }
    let mut there: HashMap<u64, Vec<Option<i128>>> = HashMap::new();
    for g in now {
        there.entry(g.hash).or_default().push(g.time_ns);
    }
    for f in was.iter().filter(|f| copies[&f.hash] == 1) {
        let Some(old) = f.time_ns else {
            continue;
        };
        let times = there.get(&f.hash).map_or(&[][..], Vec::as_slice);
        let nearest = times
            .iter()
            .flatten()
            .min_by_key(|&&t| (t - old).abs())
            .copied();
        match nearest {
            Some(t) if (t - old).abs() <= MOVED_NS => {}
            _ if times.iter().any(Option::is_none) => {}
            found => return Some((old, found)),
        }
    }
    None
}

/// Where [`audio_times`] looks, as span starts: where `audio` decodes, from the head (see
/// [`audio::spread`]). The head alone in a file without a Duration, or of none.
fn spans(duration_ns: Option<u64>) -> Vec<i128> {
    let placed = duration_ns.map_or_else(Vec::new, |d| audio::spread(d, SPAN_NS, 0));
    if placed.is_empty() {
        return vec![0];
    }
    placed.iter().map(|&(start, _)| i128::from(start)).collect()
}

/// Per track number: frame count, byte total and a hash of every frame in order.
fn digests(f: File) -> Result<HashMap<u64, (u64, u64, u64)>> {
    let mut state: HashMap<u64, (u64, u64, DefaultHasher)> = HashMap::new();
    blocks::each_frame(f.r, f.layout, |track, frame| {
        let (count, bytes, h) = state.entry(track).or_default();
        *count += 1;
        *bytes += frame.len() as u64;
        frame.hash(h);
    })?;
    Ok(state
        .into_iter()
        .map(|(track, (count, bytes, h))| (track, (count, bytes, h.finish())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_out_of_place_is_the_first_one_moved_or_missing() {
        const MS: i128 = 1_000_000;
        let frame = |ms: i128, hash: u64| Timed {
            time_ns: Some(ms * MS),
            hash,
        };
        let was = [frame(0, 1), frame(32, 2), frame(64, 3)];
        // Kept, to within the 2 ms a remux may move a frame.
        assert_eq!(
            first_moved(&was, &[frame(1, 1), frame(30, 2), frame(64, 3)]),
            None
        );
        // Moved 30 ms, and the first of them named.
        let moved = [frame(30, 1), frame(62, 2), frame(94, 3)];
        assert_eq!(first_moved(&was, &moved), Some((0, Some(30 * MS))));
        // Gone, and only frame 3 is there.
        assert_eq!(first_moved(&was, &[frame(64, 3)]), Some((0, None)));
        // Bytes the span holds twice (digital silence) say nothing, nor does a frame of its own
        // with no time.
        let silence = [
            frame(0, 9),
            frame(32, 9),
            Timed {
                time_ns: None,
                hash: 4,
            },
        ];
        assert_eq!(first_moved(&silence, &[frame(500, 9)]), None);
        // A copy in the remux with no time to read passes; with a time, it is checked.
        let untimed = [
            Timed {
                time_ns: None,
                hash: 1,
            },
            frame(32, 2),
            frame(64, 3),
        ];
        assert_eq!(first_moved(&was, &untimed), None);
    }

    #[test]
    fn audio_is_checked_at_the_head_and_spread_over_the_file() {
        const S: i128 = 1_000_000_000;
        let starts = |d: u64| {
            spans(Some(d * 1_000_000_000))
                .iter()
                .map(|s| s / S)
                .collect::<Vec<_>>()
        };
        assert_eq!(starts(1440), [0, 288, 576, 864, 1152, 1380]);
        // 30 s: 5 s spans from 0 and every 6 s; nothing a minute before the end.
        assert_eq!(starts(30), [0, 6, 12, 18, 24]);
        assert_eq!(starts(8), [0], "1.6 s overlaps the head");
        assert_eq!(starts(3), [0], "shorter than a span");
        assert_eq!(spans(None), [0]);
    }

    #[test]
    fn integers_compare_by_value_and_floats_as_floats() {
        // DisplayWidth (54B0) 1920 in 2 bytes and in 4.
        let narrow = [0x54, 0xB0, 0x82, 0x07, 0x80];
        let wide = [0x54, 0xB0, 0x84, 0x00, 0x00, 0x07, 0x80];
        assert!(same_properties(Some(&narrow), Some(&wide)));
        let other = [0x54, 0xB0, 0x82, 0x07, 0x81];
        assert!(!same_properties(Some(&narrow), Some(&other)));
        // SamplingFrequency (B5) 48000 as a 4-byte and an 8-byte float.
        let f4 = [&[0xB5, 0x84][..], &48000f32.to_be_bytes()].concat();
        let f8 = [&[0xB5, 0x88][..], &48000f64.to_be_bytes()].concat();
        assert!(same_properties(Some(&f4), Some(&f8)));
        assert!(!same_properties(Some(&f4), None));
    }

    #[test]
    fn video_and_audio_values_at_their_defaults_say_what_none_says() {
        let uint = |id: &[u8], v: u8| el(id, &[v]);
        let float = |id: &[u8], v: f32| el(id, &v.to_be_bytes());
        let same = |was: &[Vec<u8>], now: &[Vec<u8>]| {
            same_properties(Some(&was.concat()), Some(&now.concat()))
        };
        // ffmpeg writes Channels (9F) 1 and SamplingFrequency (B5) 8000; mkvmerge leaves both out.
        let depth = uint(&[0x62, 0x64], 16);
        let mono = [uint(&[0x9F], 1), float(&[0xB5], 8000.0), depth.clone()];
        assert!(same(&mono, std::slice::from_ref(&depth)));
        assert!(!same(
            &[uint(&[0x9F], 2), depth.clone()],
            std::slice::from_ref(&depth)
        ));
        assert!(!same(&[float(&[0xB5], 48000.0)], &[]));
        // OutputSamplingFrequency (78B5) is the SamplingFrequency unless it says otherwise.
        let rate = float(&[0xB5], 48000.0);
        assert!(same(
            &[rate.clone(), float(&[0x78, 0xB5], 48000.0)],
            std::slice::from_ref(&rate)
        ));
        assert!(!same(
            &[rate.clone(), float(&[0x78, 0xB5], 96000.0)],
            std::slice::from_ref(&rate)
        ));
        // mkvmerge writes DisplayWidth (54B0) and DisplayHeight (54BA) as the pixel size (B0, BA),
        // less any crop (54CC left, 54DD right), when DisplayUnit (54B2) is pixels; ffmpeg leaves
        // them out.
        let pixels = [uint(&[0xB0], 64), uint(&[0xBA], 36)];
        let shown = |w: u8, h: u8| [uint(&[0x54, 0xB0], w), uint(&[0x54, 0xBA], h)];
        assert!(same(&pixels, &[pixels.as_slice(), &shown(64, 36)].concat()));
        assert!(!same(
            &pixels,
            &[pixels.as_slice(), &shown(85, 36)].concat()
        ));
        let cropped = [
            pixels.as_slice(),
            &[uint(&[0x54, 0xCC], 2), uint(&[0x54, 0xDD], 2)],
        ]
        .concat();
        assert!(same(
            &cropped,
            &[cropped.as_slice(), &shown(60, 36)].concat()
        ));
        assert!(!same(
            &cropped,
            &[cropped.as_slice(), &shown(64, 36)].concat()
        ));
        // In another unit (3: an aspect ratio), a DisplayWidth is no pixel count.
        let ratio = [pixels.as_slice(), &[uint(&[0x54, 0xB2], 3)]].concat();
        assert!(!same(&ratio, &[ratio.as_slice(), &shown(64, 36)].concat()));
        assert!(same(
            &[pixels.as_slice(), &[uint(&[0x54, 0xB2], 0)]].concat(),
            &pixels
        ));
    }

    #[test]
    fn properties_that_do_not_parse_compare_as_bytes() {
        // A DisplayWidth whose size says 5 bytes, with 1 there.
        let cut = |b: u8| [0x54, 0xB0, 0x85, b];
        assert!(!same_properties(Some(&cut(1)), Some(&cut(2))));
        assert!(same_properties(Some(&cut(1)), Some(&cut(1))));
        assert!(!same_properties(
            Some(&cut(1)),
            Some(&[0x54, 0xB0, 0x81, 1])
        ));
    }

    #[test]
    fn a_progressive_flag_mkvmerge_leaves_out_is_no_difference() {
        // FlagInterlaced (9A): 2 is progressive, 1 interlaced, 0 (the default) undetermined.
        let flag = |v: u8| el(&[0x9A], &[v]);
        let pixels = el(&[0xB0], &[64]);
        let same = |was: &[Vec<u8>], now: &[Vec<u8>]| {
            same_properties(Some(&was.concat()), Some(&now.concat()))
        };
        let alone = std::slice::from_ref(&pixels);
        assert!(same(&[flag(2), pixels.clone()], alone));
        assert!(same(&[flag(2), pixels.clone()], &[flag(0), pixels.clone()]));
        assert!(same(&[flag(2), pixels.clone()], &[flag(2), pixels.clone()]));
        // Interlaced left out is a difference, and so is progressive made up.
        assert!(!same(&[flag(1), pixels.clone()], alone));
        assert!(!same(alone, &[flag(2), pixels.clone()]));
    }

    #[test]
    fn an_audio_default_duration_where_the_original_had_none_is_no_difference() {
        let flac = |d: Option<u64>| Track {
            kind: mkv::TRACK_TYPE_AUDIO,
            codec: "A_FLAC".into(),
            default_duration: d,
            ..track("jpn", None)
        };
        // ffmpeg 7 wrote none; mkvmerge writes the codec's frame length, FLAC's 96 ms here.
        assert_eq!(
            diffs(&flac(None), &flac(Some(96_000_000))),
            [] as [String; 0]
        );
        let differs = ["track 2: default duration differs"];
        assert_eq!(
            diffs(&flac(Some(23_219_954)), &flac(Some(96_000_000))),
            differs
        );
        assert_eq!(diffs(&flac(Some(96_000_000)), &flac(None)), differs);
        // Video keeps its own.
        let video = |d: Option<u64>| Track {
            kind: mkv::TRACK_TYPE_VIDEO,
            codec: "V_MPEG4/ISO/AVC".into(),
            ..flac(d)
        };
        assert_eq!(diffs(&video(None), &video(Some(41_708_333))), differs);
    }

    #[test]
    fn flags_left_out_read_as_their_defaults() {
        let set = Flags {
            enabled: Some(true),
            commentary: Some(false),
            ..Flags::default()
        };
        assert_eq!(flags(&set), flags(&Flags::default()));
        let commentary = Flags {
            commentary: Some(true),
            ..Flags::default()
        };
        assert_ne!(flags(&commentary), flags(&Flags::default()));
    }

    #[test]
    fn an_attached_file_compares_without_its_mime_type() {
        // FileName "a.ttf", FileMimeType, FileData "xyz".
        let file = |mime: &[u8], data: &[u8]| {
            let mut v = vec![0x46, 0x6E, 0x85];
            v.extend_from_slice(b"a.ttf");
            v.extend_from_slice(&[0x46, 0x60, 0x80 | u8::try_from(mime.len()).unwrap()]);
            v.extend_from_slice(mime);
            v.extend_from_slice(&[0x46, 0x5C, 0x80 | u8::try_from(data.len()).unwrap()]);
            v.extend_from_slice(data);
            attached(&v, 0).unwrap()
        };
        assert_eq!(
            file(b"application/x-truetype-font", b"xyz"),
            file(b"font/ttf", b"xyz")
        );
        assert_ne!(file(b"font/ttf", b"xyz"), file(b"font/ttf", b"xyZ"));
    }

    #[test]
    fn an_attached_files_uid_compares_by_value() {
        // FileName "a.ttf", FileUID: ffmpeg writes 4 in 8 bytes, mkvmerge in 1.
        let file = |uid: &[u8]| {
            let mut v = vec![0x46, 0x6E, 0x85];
            v.extend_from_slice(b"a.ttf");
            v.extend_from_slice(&[0x46, 0xAE, 0x80 | u8::try_from(uid.len()).unwrap()]);
            v.extend_from_slice(uid);
            attached(&v, 0).unwrap()
        };
        assert_eq!(file(&[0, 0, 0, 0, 0, 0, 0, 4]), file(&[4]));
        assert_ne!(file(&[0, 0, 0, 0, 0, 0, 0, 4]), file(&[5]));
    }

    fn track(language: &str, bcp47: Option<&str>) -> Track {
        Track {
            id: 2,
            number: 3,
            uid: 3,
            kind: mkv::TRACK_TYPE_SUBTITLE,
            codec: "S_TEXT/ASS".into(),
            name: None,
            language: language.into(),
            language_element: true,
            language_bcp47: bcp47.map(Into::into),
            default: true,
            forced: false,
            default_duration: None,
            codec_private: Vec::new(),
            encodings: Vec::new(),
            flags: Flags::default(),
            video: None,
            audio: None,
        }
    }

    /// What [`header`] finds between two tracks, neither one fixed.
    fn diffs(a: &Track, b: &Track) -> Vec<String> {
        let mut d = Vec::new();
        header(a, None, b, false, &mut d).unwrap();
        d
    }

    #[test]
    fn a_bcp_47_tag_added_by_a_newer_mkvmerge_is_no_difference() {
        let old = track("eng", None);
        assert_eq!(diffs(&old, &track("eng", Some("en"))), [] as [String; 0]);
        assert_eq!(
            diffs(&track("eng", Some("en")), &track("eng", Some("en-US"))),
            ["track 2: language differs"]
        );
        assert_eq!(
            diffs(&old, &track("jpn", None)),
            ["track 2: language differs"]
        );
    }

    #[test]
    fn beside_a_bcp_47_tag_the_language_element_is_no_difference() {
        // Tagged only `ja`: Language reads as Matroska's default, and mkvmerge's remux fills it in
        // from the BCP 47 tag.
        let only = Track {
            language_element: false,
            ..track("eng", Some("ja"))
        };
        assert_eq!(diffs(&only, &track("jpn", Some("ja"))), [] as [String; 0]);
        assert_eq!(
            diffs(&only, &track("jpn", Some("ja-JP"))),
            ["track 2: language differs"]
        );
        // Nor is one the file gives: mkvmerge writes Language `eng` beside `ja` back as `jpn`
        // (checked, mkvmerge v102.0).
        assert_eq!(
            diffs(&track("eng", Some("ja")), &track("jpn", Some("ja"))),
            [] as [String; 0]
        );
        // BCP 47 tags ignore case: mkvmerge writes `ja-jp` back as `ja-JP`.
        assert_eq!(
            diffs(&track("jpn", Some("ja-jp")), &track("jpn", Some("ja-JP"))),
            [] as [String; 0]
        );
    }

    #[test]
    fn language_is_compared_as_mkvmerge_reads_the_original() {
        // As `mkvmerge -J` reads the original, so the remux has it (checked, mkvmerge v102.0): a
        // tagless Language lowercase and bibliographic (`fra`, `Fre`, `fr` as `fre`; `xyz` as
        // `und`), and a tag in its canonical form (`iw` as `he`, `zh-yue` as `yue`).
        let read_as = |iso: &str, bcp47: Option<&str>| {
            bcp47.map_or_else(
                || Language::Iso639_2(iso.into()),
                |tag| Language::Bcp47(tag.into()),
            )
        };
        let check = |t: &Track, read: &Language, u: &Track| {
            let mut d = Vec::new();
            header(t, Some(read), u, false, &mut d).unwrap();
            d
        };
        let none = [] as [String; 0];
        let differs = ["track 2: language differs"];
        // Tagless: the remux's Language is mkvmerge's reading; the tag it adds is no difference.
        let fre = read_as("fre", None);
        assert_eq!(
            check(&track("fra", None), &fre, &track("fre", Some("fr"))),
            none
        );
        assert_eq!(
            check(&track("fr", None), &fre, &track("fre", Some("fr"))),
            none
        );
        assert_eq!(
            check(&track("fra", None), &fre, &track("ger", Some("de"))),
            differs
        );
        let und = read_as("und", None);
        assert_eq!(
            check(&track("xyz", None), &und, &track("und", Some("und"))),
            none
        );
        // Tagged: the remux's tag is mkvmerge's reading, case aside, and Language beside it is not
        // compared.
        let he = read_as("jpn", Some("he"));
        assert_eq!(
            check(&track("jpn", Some("iw")), &he, &track("heb", Some("he"))),
            none
        );
        let yue = read_as("jpn", Some("yue"));
        assert_eq!(
            check(
                &track("jpn", Some("zh-yue")),
                &yue,
                &track("chi", Some("yue"))
            ),
            none
        );
        let ja = read_as("jpn", Some("ja"));
        assert_eq!(
            check(&track("jpn", Some("ja")), &ja, &track("jpn", Some("en"))),
            differs
        );
    }

    #[test]
    fn a_text_tracks_default_duration_is_compared_as_its_lines_durations() {
        let timed = |d: u64, t: &Track| Track {
            default_duration: Some(d),
            ..t.clone()
        };
        // 23.976 fps.
        let frame = 41_708_333;
        let ass = track("eng", None);
        let differs = ["track 2: default duration differs"];
        // Left out of a text track's header, it is in its lines, below; kept, it is the same.
        assert_eq!(diffs(&timed(frame, &ass), &ass), [] as [String; 0]);
        assert_eq!(
            diffs(&timed(frame, &ass), &timed(40_000_000, &ass)),
            differs
        );
        assert_eq!(diffs(&ass, &timed(frame, &ass)), differs);
        // An image track's blocks are compared by their frames only, not their durations.
        let pgs = Track {
            codec: "S_HDMV/PGS".into(),
            ..track("eng", None)
        };
        assert_eq!(diffs(&timed(frame, &pgs), &pgs), differs);
        // A line with no duration of its own, as a remux writes it: its track's DefaultDuration in
        // whole milliseconds, rounded to the nearest (mkvmerge 102 makes 42 ms of 41.708333 and of
        // 41.5, 41 of 41.499999).
        let line = |duration| Block {
            track: 3,
            position: 0,
            timestamp: 0,
            duration,
            payload: b"0,0,D,,0,0,0,,line".to_vec(),
        };
        let lines =
            |duration, default| events(Kind::Ass, &[line(duration)], 1_000_000, default, true);
        let remuxed = |ms| lines(Some(ms), None);
        let original = |ns| lines(None, as_remuxed(Some(ns), 1_000_000));
        assert_eq!(original(frame), remuxed(42));
        assert_ne!(original(frame), remuxed(41));
        assert_eq!(original(41_500_000), remuxed(42));
        assert_eq!(original(41_499_999), remuxed(41));
        // A remux whose line has no duration and no DefaultDuration to take one from.
        assert_ne!(original(frame), lines(None, None));
    }

    #[test]
    fn read_orders_compare_by_rank() {
        let block = |order: u32, ms: i64| Block {
            track: 3,
            position: 0,
            timestamp: ms,
            duration: Some(1000),
            payload: format!("{order},0,D,,0,0,0,,line {ms}").into_bytes(),
        };
        let numbered = |orders: [u32; 3]| {
            let blocks: Vec<Block> = orders
                .iter()
                .zip([0, 1000, 2000])
                .map(|(&o, ms)| block(o, ms))
                .collect();
            events(Kind::Ass, &blocks, 1_000_000, None, true)
        };
        assert_eq!(numbered([7, 3, 9]), numbered([1, 0, 2]));
        assert_ne!(numbered([7, 3, 9]), numbered([0, 1, 2]));
    }

    #[test]
    fn a_rewritten_srt_track_compares_without_crs() {
        let rows = |text: &str, rewritten| {
            let b = Block {
                track: 3,
                position: 0,
                timestamp: 0,
                duration: Some(1000),
                payload: text.as_bytes().to_vec(),
            };
            events(Kind::Srt, &[b], 1_000_000, None, rewritten)
        };
        assert_eq!(rows("Two\r\nrows", true), rows("Two\nrows", true));
        assert_ne!(rows("Two\r\nrows", false), rows("Two\nrows", false));
    }

    /// One element whose size fits a 1-byte vint.
    fn el(id: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.push(0x80 | u8::try_from(data.len()).unwrap());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn chapters_compare_without_crc_or_void_and_integers_by_value() {
        // An EditionEntry with one ChapterAtom (UID, start time), as ffmpeg writes it (a CRC-32
        // first, the time in 1 byte) and as mkvmerge does (no CRC, the time in 2 bytes).
        let atom = |time: &[u8]| {
            el(
                &[0xB6],
                &[el(&[0x73, 0xC4], &[7]), el(&[0x91], time)].concat(),
            )
        };
        let ffmpeg = el(
            &[0x45, 0xB9],
            &[el(&[0xBF], &[1, 2, 3, 4]), atom(&[5])].concat(),
        );
        let mkvmerge = el(&[0x45, 0xB9], &atom(&[0, 5]));
        assert_eq!(
            values(&ffmpeg, CHAPTER_MASTERS),
            values(&mkvmerge, CHAPTER_MASTERS)
        );
        let later = el(&[0x45, 0xB9], &atom(&[6]));
        assert_ne!(
            values(&ffmpeg, CHAPTER_MASTERS),
            values(&later, CHAPTER_MASTERS)
        );
    }

    #[test]
    fn an_edition_uid_where_the_original_had_none_is_no_difference() {
        // EditionEntry (45B9) with one ChapterAtom, and EditionUID (45BC) `uid` if there is one.
        let edition = |uid: Option<u8>| {
            let atom = el(&[0xB6], &el(&[0x73, 0xC4], &[7]));
            let uid = uid.map_or(Vec::new(), |u| el(&[0x45, 0xBC], &[u]));
            el(&[0x45, 0xB9], &[uid, atom].concat())
        };
        let same = |was: &[Option<u8>], now: &[Option<u8>]| {
            let chapters = |uids: &[Option<u8>]| {
                values(
                    &uids.iter().flat_map(|&u| edition(u)).collect::<Vec<u8>>(),
                    CHAPTER_MASTERS,
                )
            };
            same_chapters(chapters(was), chapters(now), false)
        };
        // mkvmerge gives each edition one; ffmpeg writes none.
        assert!(same(&[None], &[Some(9)]));
        assert!(same(&[Some(9)], &[Some(9)]));
        assert!(!same(&[Some(9)], &[Some(8)]));
        assert!(!same(&[Some(9)], &[None]));
        // Edition by edition.
        assert!(same(&[Some(9), None], &[Some(9), Some(3)]));
        assert!(!same(&[None, Some(9)], &[Some(3), Some(8)]));
        // No chapters on either side, or on one.
        assert!(same_chapters(None, None, false));
        assert!(!same_chapters(None, chapters_of_one(), false));
    }

    /// One edition with one atom, as compared.
    fn chapters_of_one() -> Option<Vec<(u32, Value)>> {
        values(
            &el(&[0x45, 0xB9], &el(&[0xB6], &el(&[0x73, 0xC4], &[7]))),
            CHAPTER_MASTERS,
        )
    }

    /// An edition of atoms, each (ChapterUID, ChapterTimeStart), with EditionUID `edition` if any.
    fn edition_of(edition: Option<u8>, atoms: &[(u8, u8)]) -> Vec<u8> {
        let uid = edition.map_or(Vec::new(), |u| el(&[0x45, 0xBC], &[u]));
        let atoms = atoms
            .iter()
            .flat_map(|&(uid, start)| {
                el(
                    &[0xB6],
                    &[el(&[0x73, 0xC4], &[uid]), el(&[0x91], &[start])].concat(),
                )
            })
            .collect::<Vec<u8>>();
        el(&[0x45, 0xB9], &[uid, atoms].concat())
    }

    #[test]
    fn chapters_mkvmerge_renumbered_compare_without_their_uids() {
        let same = |was: &[u8], now: &[u8], renumbered| {
            same_chapters(
                values(was, CHAPTER_MASTERS),
                values(now, CHAPTER_MASTERS),
                renumbered,
            )
        };
        // ffmpeg's chapters 1 and 2, and mkvmerge's remux of them: new UIDs, and an EditionUID.
        let ffmpeg = edition_of(None, &[(1, 0), (2, 15)]);
        let remux = edition_of(Some(70), &[(81, 0), (92, 15)]);
        assert!(same(&ffmpeg, &remux, true));
        assert!(!same(&ffmpeg, &remux, false));
        // The times still count, and the editions' UIDs where both had one.
        assert!(!same(
            &ffmpeg,
            &edition_of(Some(70), &[(81, 0), (92, 16)]),
            true
        ));
        let both = edition_of(Some(5), &[(1, 0)]);
        assert!(same(&both, &edition_of(Some(6), &[(81, 0)]), true));
        assert!(!same(&both, &edition_of(Some(6), &[(81, 0)]), false));
        // Nested atoms are renumbered too.
        let nested = |outer: u8, inner: u8| {
            el(
                &[0x45, 0xB9],
                &el(
                    &[0xB6],
                    &[
                        el(&[0x73, 0xC4], &[outer]),
                        el(&[0xB6], &el(&[0x73, 0xC4], &[inner])),
                    ]
                    .concat(),
                ),
            )
        };
        assert!(same(&nested(1, 2), &nested(3, 4), true));
        assert!(!same(&nested(1, 2), &nested(3, 4), false));
    }

    #[test]
    fn mkvmerge_renumbers_the_chapters_of_a_file_another_program_wrote() {
        assert!(renumbers_chapters(Some("Lavf61.7.100")));
        assert!(renumbers_chapters(Some("Lavf")));
        assert!(renumbers_chapters(Some("HandBrake 1.6.1 2023012300")));
        assert!(!renumbers_chapters(Some(
            "mkvmerge v103.0 ('The Darkness') 64-bit"
        )));
        assert!(!renumbers_chapters(Some(" MKVMerge v9.0")));
        assert!(!renumbers_chapters(Some("no_variable_data")));
        // None to go by: mkvmerge keeps them.
        assert!(!renumbers_chapters(None));
    }

    #[test]
    fn chapter_languages_compare_as_players_read_them() {
        // A ChapterAtom whose ChapterDisplay (80) has ChapString (85) "A", ChapLanguages (437C)
        // `iso` and ChapLanguageBCP47s (437D) `tags`.
        let display = |iso: &[&[u8]], tags: &[&[u8]]| {
            let langs: Vec<u8> = iso
                .iter()
                .map(|l| el(&[0x43, 0x7C], l))
                .chain(tags.iter().map(|t| el(&[0x43, 0x7D], t)))
                .flatten()
                .collect();
            let display = el(&[0x80], &[el(&[0x85], b"A"), langs].concat());
            values(
                &el(
                    &[0x45, 0xB9],
                    &el(&[0xB6], &[el(&[0x73, 0xC4], &[7]), display].concat()),
                ),
                CHAPTER_MASTERS,
            )
        };
        let same = |was, now| same_chapters(was, now, false);
        // ffmpeg writes ChapLanguage alone; mkvmerge writes it lowercase and bibliographic, and
        // adds the tag from it (checked, mkvmerge v103.0).
        assert!(same(display(&[b"und"], &[]), display(&[b"und"], &[b"und"])));
        assert!(same(display(&[b"eng"], &[]), display(&[b"eng"], &[b"en"])));
        assert!(same(display(&[b"fra"], &[]), display(&[b"fre"], &[b"fr"])));
        assert!(same(display(&[b"ENG"], &[]), display(&[b"eng"], &[b"en"])));
        assert!(same(
            display(&[b"eng\0"], &[]),
            display(&[b"eng"], &[b"en"])
        ));
        assert!(same(display(&[b"ara"], &[]), display(&[b"ara"], &[b"ar"])));
        assert!(same(display(&[b"haw"], &[]), display(&[b"haw"], &[b"haw"])));
        // Players go by the tag: one of another language is a difference.
        assert!(!same(display(&[b"eng"], &[]), display(&[b"eng"], &[b"ja"])));
        assert!(!same(display(&[b"und"], &[]), display(&[b"und"], &[b"en"])));
        assert!(!same(
            display(&[b"eng"], &[b"en"]),
            display(&[b"eng"], &[b"ja"])
        ));
        assert!(!same(display(&[b"eng"], &[]), display(&[b"jpn"], &[b"ja"])));
        // A tag alone, and the ChapLanguage mkvmerge writes from it.
        assert!(same(display(&[], &[b"ja"]), display(&[b"jpn"], &[b"ja"])));
        // No language at all is Matroska's `eng`: mkvmerge writes `eng`, tagged `en`.
        assert!(same(display(&[], &[]), display(&[b"eng"], &[b"en"])));
        assert!(!same(display(&[], &[]), display(&[b"jpn"], &[b"ja"])));
        // Every language of the display counts.
        let both: &[&[u8]] = &[b"jpn", b"eng"];
        assert!(same(display(both, &[]), display(both, &[b"ja", b"en"])));
        assert!(!same(display(both, &[]), display(both, &[b"en"])));
    }

    #[test]
    fn chapter_atoms_compare_in_their_order() {
        // An ordered edition plays its atoms in order: the same atoms swapped are a difference.
        let atom = |uid: u8| el(&[0xB6], &el(&[0x73, 0xC4], &[uid]));
        let one = el(&[0x45, 0xB9], &[atom(1), atom(2)].concat());
        let other = el(&[0x45, 0xB9], &[atom(2), atom(1)].concat());
        assert_ne!(
            values(&one, CHAPTER_MASTERS),
            values(&other, CHAPTER_MASTERS)
        );
    }

    #[test]
    fn chapters_that_do_not_parse_are_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c");
        // A Chapters element whose child claims more bytes than there are.
        std::fs::write(&path, el(&[0x10, 0x43, 0xA7, 0x70], &[0x45, 0xB9, 0x88, 1])).unwrap();
        let err = chapters(&Reader::open(&path).unwrap(), 0).unwrap_err();
        assert!(
            err.to_string().contains("chapters that do not parse"),
            "{err}"
        );
    }

    #[test]
    fn a_header_compares_as_bytes() {
        let latin1 = [b"Style: Caf".as_slice(), &[0xE9]].concat();
        assert_ne!(script(&latin1), script("Style: Caf\u{FFFD}".as_bytes()));
        assert_eq!(script(b"A\r\nB\r\n\r\n"), script(b"A\nB"));
    }

    #[test]
    fn an_attachments_crc_is_no_attached_file() {
        let file = el(
            &[0x61, 0xA7],
            &[el(&[0x46, 0x6E], b"a.ttf"), el(&[0x46, 0x5C], b"xyz")].concat(),
        );
        let dir = tempfile::tempdir().unwrap();
        let read = |children: &[u8]| {
            let path = dir.path().join("a");
            std::fs::write(&path, el(&[0x19, 0x41, 0xA4, 0x69], children)).unwrap();
            attached_files(&Reader::open(&path).unwrap(), 0).unwrap()
        };
        let with_crc = read(&[el(&[0xBF], &[1, 2, 3, 4]), file.clone()].concat());
        assert_eq!(with_crc, read(&file));
        assert_eq!(with_crc.len(), 1);
    }

    #[test]
    fn bytes_that_are_not_utf8_compare_as_bytes() {
        // Latin-1 é, and what a lossy rewrite would make of it.
        let line = |bytes: &[u8]| {
            let b = Block {
                track: 3,
                position: 0,
                timestamp: 0,
                duration: Some(1000),
                payload: [b"0,0,D,,0,0,0,,caf".as_slice(), bytes].concat(),
            };
            events(Kind::Ass, &[b], 1_000_000, None, true)
        };
        assert_ne!(line(&[0xE9]), line("\u{FFFD}".as_bytes()));
    }

    #[test]
    fn an_ass_header_compares_without_crlf_or_blank_lines() {
        assert_eq!(
            script(b"[Script Info]\r\nA: 1\r\n\r\n"),
            script(b"[Script Info]\nA: 1\n")
        );
        // As K-On!! E09's header comes back: a blank line more before Aegisub's extradata.
        assert_eq!(
            script(b"[Events]\r\nFormat: Text\r\n\r\n[Aegisub Extradata]\r\nData: 0\r\n"),
            script(b"[Events]\r\nFormat: Text\r\n\r\n\r\n[Aegisub Extradata]\r\nData: 0\r\n")
        );
        // Its last line's trailing blanks, which the writer drops.
        assert_eq!(script(b"Data: 0  \t\r\n"), script(b"Data: 0"));
        assert_ne!(script(b"A: 1\n"), script(b"A: 2\n"));
        assert_ne!(script(b"A: 1\nB: 2\n"), script(b"A: 1 B: 2\n"));
    }
}
