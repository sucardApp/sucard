//! 数据层：扫描根目录下的 .md / .txt，解析任务行，读写文件和排序索引。
//!
//! 每条内容在文件里是一行标准 Markdown 任务：
//!   - [ ] 短视频开头三秒要抛问题 #灵感 ^k3f9a2
//! `[x]` 表示已用 / 已完成，`#标签` 写在正文里，行尾 `^id` 用来在左右两栏对应同一条。
//! 文件里的其他行（标题、段落、普通列表）原样保留，不会被改动。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const INBOX: &str = "收件箱.md";
pub const VAULT: &str = "保管箱.md";
/// 自带的使用说明（第一次使用时生成，也可以在 UI设置 里重新生成）
pub const GUIDE: &str = "使用说明.md";
const GUIDE_TEXT: &str = include_str!("../assets/使用说明.md");
/// 固定文档：总是存在，不能删除、不能改名（缺了启动时自动建）
pub const FIXED_DOCS: [&str; 2] = [INBOX, VAULT];

/// 是不是固定文档（收件箱、保管箱）
pub fn is_fixed_doc(rel: &Path) -> bool {
    FIXED_DOCS.iter().any(|f| rel == Path::new(f))
}
const STATE_DIR: &str = ".sucard";
const STATE_FILE: &str = "state.json";
/// 回收站：.sucard/回收站/ 下放被删的文档，index.json 记原位置和删除时间
const TRASH_DIR: &str = "回收站";
const TRASH_INDEX: &str = "index.json";

/// 回收站里的一个文档
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TrashEntry {
    /// 回收站里的文件名（时间戳_原文件名，避免重名）
    pub file: String,
    /// 原来的位置（相对数据目录）
    pub original: String,
    /// 删除时间（Unix 秒）
    pub deleted_at: u64,
    /// 删除时里面有几条素材
    #[serde(default)]
    pub items: usize,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    /// 行首部分，例如 "- " 或 "  * "，写回时保持原样
    pub prefix: String,
    pub done: bool,
    /// 正文，含 #标签，不含 ^id
    pub text: String,
    /// 相对根目录的文件路径
    pub file: PathBuf,
}

impl Item {
    pub fn tags(&self) -> Vec<String> {
        parse_tags(&self.text)
    }
    pub fn body(&self) -> String {
        strip_tags(&self.text)
    }
    pub fn has_tag(&self, tag: &str) -> bool {
        let child = format!("{tag}/");
        self.tags().iter().any(|t| t == tag || t.starts_with(&child))
    }
    /// 写回文件的文字：首行是任务行（带 ^id），之后每行按列表内容缩进挂在下面
    fn to_line(&self) -> String {
        let mark = if self.done { 'x' } else { ' ' };
        let text = clean_text(&self.text);
        let mut lines = text.lines();
        let first = lines.next().unwrap_or("");
        let mut out = if first.is_empty() {
            format!("{}[{}] ^{}", self.prefix, mark, self.id)
        } else {
            format!("{}[{}] {} ^{}", self.prefix, mark, first, self.id)
        };
        let indent = " ".repeat(self.prefix.chars().count());
        for l in lines {
            out.push('\n');
            out.push_str(&indent);
            out.push_str(l);
        }
        out
    }
}

#[derive(Clone, Debug)]
enum Line {
    Raw(String),
    Item(String),
}

#[derive(Clone, Debug)]
pub struct Doc {
    pub path: PathBuf,
    lines: Vec<Line>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Prefs {
    #[serde(default)]
    pub hide_done: bool,
    #[serde(default)]
    pub left_by_tag: bool,
    #[serde(default)]
    /// 右栏 TODO 显示「已完成」（否则是「手动排序」的待办）
    pub right_done: bool,
    #[serde(default)]
    pub ui: UiPrefs,
    /// 左栏「按标签」里折叠起来的父标签
    #[serde(default)]
    pub collapsed_tags: Vec<String>,
    /// 不放入 TODO（右栏不显示）的文档，存相对路径
    #[serde(default)]
    pub not_in_todo: Vec<String>,
}

/// 界面外观设置。都是“改一次就生效”的配置，不会每帧重新计算。
/// 旧的 state.json 里没有这些字段，缺哪个就用默认值。
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct UiPrefs {
    /// 0 跟随系统，1 浅色，2 深色，3 护眼，5 樱米，6 薰衣草，7 深海，8 薄荷，9 羊皮纸（4 是已移除的护眼（浅），读到时改成樱米）
    pub theme: u8,
    /// 强调色预设的序号，0 是 egui 默认的蓝色；等于预设个数（7）时用 custom_accent
    pub accent: u8,
    /// 自定义强调色（RGB）
    pub custom_accent: [u8; 3],
    /// 正文字号（egui 默认 12.5），其他文字按比例缩放
    pub font_size: f32,
    /// 小号字（标签、文件名、提示等）是正文的百分之几
    pub small_pct: f32,
    /// 卡片、左栏块之间的间距
    pub gap: f32,
    /// 卡片、左栏块、按钮的圆角
    pub radius: f32,
    /// 左栏块的高度，以普通一行为单位
    pub nav_rows: f32,
    /// 输入框光标是否闪烁。闪烁时程序在前台会一直重画（约占一个核 5%），默认关闭
    pub cursor_blink: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        UiPrefs {
            theme: 0,
            accent: 0,
            custom_accent: [1, 86, 151],
            font_size: 12.5,
            small_pct: 85.0,
            gap: 4.0,
            radius: 6.0,
            nav_rows: 2.0,
            cursor_blink: false,
        }
    }
}

impl UiPrefs {
    /// 把值限制在设置面板允许的范围内（防止 state.json 被手改成离谱的值）
    pub fn clamped(mut self) -> Self {
        self.theme = self.theme.min(9);
        // 4「护眼（浅）」已移除，换成同样是米色底的「樱米」
        if self.theme == 4 {
            self.theme = 5;
        }
        self.font_size = if self.font_size.is_finite() { self.font_size.clamp(10.0, 22.0) } else { 12.5 };
        self.small_pct = if self.small_pct.is_finite() { self.small_pct.clamp(60.0, 100.0) } else { 85.0 };
        self.gap = if self.gap.is_finite() { self.gap.clamp(0.0, 16.0) } else { 4.0 };
        self.radius = if self.radius.is_finite() { self.radius.clamp(0.0, 12.0) } else { 6.0 };
        self.nav_rows = if self.nav_rows.is_finite() { self.nav_rows.clamp(1.0, 3.0) } else { 2.0 };
        self
    }
}

#[derive(Serialize, Deserialize, Default)]
struct StateFile {
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    prefs: Prefs,
}

pub struct Store {
    pub root: PathBuf,
    pub docs: Vec<Doc>,
    pub items: HashMap<String, Item>,
    /// 待办区的手动排序（跨文件）
    pub order: Vec<String>,
    pub prefs: Prefs,
    pub error: Option<String>,
    /// 回收站列表（启动和操作时读写，平时缓存在内存，最近删除的在前）
    pub trash: Vec<TrashEntry>,
    rng: u64,
}

// ---------- 解析工具 ----------

/// 一个词如果是标签（#xxx），返回去掉 # 和末尾标点后的标签名
pub fn tag_of_word(w: &str) -> Option<&str> {
    let t = w.strip_prefix('#')?;
    let t = t.trim_end_matches(|c: char| ",.;:!?，。；：！？、".contains(c));
    let t = t.trim_matches('/');
    if t.is_empty() || t.starts_with('#') {
        None
    } else {
        Some(t)
    }
}

/// 逐词判断是否在 `%%备注%%` 里（带 %% 记号的词也算）。要按顺序对每个词调用
#[derive(Default)]
struct CommentTracker {
    inside: bool,
}

impl CommentTracker {
    fn is_comment(&mut self, word: &str) -> bool {
        let n = word.matches("%%").count();
        let comment = self.inside || n > 0;
        if n % 2 == 1 {
            self.inside = !self.inside;
        }
        comment
    }
}

/// 文字里的标签（去重，按出现顺序）。`%%备注%%` 里的 #标签 不算
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut c = CommentTracker::default();
    for w in text.split_whitespace() {
        if c.is_comment(w) {
            continue;
        }
        if let Some(t) = tag_of_word(w) {
            if !out.iter().any(|x| x == t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// 去掉满足条件的词（备注里的词不动），去掉后变空的行也去掉
fn drop_words(text: &str, drop: impl Fn(&str) -> bool) -> String {
    let mut c = CommentTracker::default();
    text.lines()
        .map(|l| keep_words(l, |w| c.is_comment(w) || !drop(w)))
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 确保文字带上某个标签：已有这个标签或它的子标签（#资料/AI 之于 资料）就不动，否则加到末尾
pub fn ensure_tag(text: &str, tag: &str) -> String {
    let child = format!("{tag}/");
    if parse_tags(text).iter().any(|t| t == tag || t.starts_with(&child)) {
        return text.trim().to_string();
    }
    push_to_first_line(text, &format!("#{tag}"))
}

/// 在一行文字里切换某个标签：已有就去掉，没有就加到末尾（后面留一个空格方便接着打字）
pub fn toggle_tag(text: &str, tag: &str) -> String {
    let mut s = if parse_tags(text).iter().any(|t| t == tag) {
        drop_words(text, |w| tag_of_word(w) == Some(tag))
    } else {
        push_to_first_line(text, &format!("#{tag}"))
    };
    // 单行时末尾留个空格，方便接着打字
    if !s.is_empty() && !s.contains('\n') {
        s.push(' ');
    }
    s
}

/// 一行里只保留满足条件的词，行首缩进原样保留
fn keep_words(line: &str, mut keep: impl FnMut(&str) -> bool) -> String {
    let lead = &line[..line.len() - line.trim_start().len()];
    let words: Vec<&str> = line.split_whitespace().filter(|w| keep(w)).collect();
    if words.is_empty() {
        String::new()
    } else {
        format!("{lead}{}", words.join(" "))
    }
}

/// 按 `%%备注%%` 切段：返回 (字节范围, 是否备注)。备注可以跨行；
/// 只有开头 `%%` 没有收尾的，一直算到结尾（和 Obsidian 一样）。`%%` 记号本身算在备注里。
pub fn split_comments(text: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        let Some(open) = text[pos..].find("%%").map(|i| pos + i) else {
            out.push((pos..text.len(), false));
            break;
        };
        if open > pos {
            out.push((pos..open, false));
        }
        let end = text[open + 2..].find("%%").map(|i| open + 2 + i + 2).unwrap_or(text.len());
        out.push((open..end, true));
        pos = end;
    }
    out
}

/// 找出文字里的网址（http://、https://、www. 开头），返回字节范围。
/// 网址只含 ASCII 字符，遇到空白或中文就结束（中文里贴网址常常不留空格）；
/// 末尾的标点、不配对的右括号、备注的 %% 不算在网址里。
pub fn find_urls(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let Some((off, prefix)) = ["https://", "http://", "www."]
            .iter()
            .filter_map(|p| rest.find(p).map(|o| (o, *p)))
            .min_by_key(|(o, p)| (*o, std::cmp::Reverse(p.len())))
        else {
            break;
        };
        let start = i + off;
        // 前面紧挨着字母数字的不算（比如 abcwww.x）
        if text[..start].chars().last().is_some_and(|c| c.is_ascii_alphanumeric()) {
            i = start + 1;
            continue;
        }
        let mut end = start;
        for (j, c) in text[start..].char_indices() {
            if c.is_ascii_graphic() && !"<>\"'`".contains(c) {
                end = start + j + c.len_utf8();
            } else {
                break;
            }
        }
        loop {
            let url = &text[start..end];
            let Some(last) = url.chars().last() else { break };
            let unbalanced = |open: char, close: char| {
                last == close && url.matches(close).count() > url.matches(open).count()
            };
            if ".,;:!?%".contains(last) || unbalanced('(', ')') || unbalanced('[', ']') {
                end -= 1;
            } else {
                break;
            }
        }
        if end > start + prefix.len() {
            out.push(start..end);
        }
        i = end.max(start + 1);
    }
    out
}

/// 找出 `==标记==`（Obsidian 的高亮写法），返回包含记号的字节范围。
/// 只在同一行里配对（落单的 == 不算），空标记不算，落在 `%%备注%%` 里的不算。
pub fn find_marks(text: &str) -> Vec<std::ops::Range<usize>> {
    let comments: Vec<_> =
        split_comments(text).into_iter().filter(|(_, c)| *c).map(|(r, _)| r).collect();
    let mut out = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let mut pos = 0;
        while let Some(o) = line[pos..].find("==") {
            let open = pos + o;
            let Some(c) = line[open + 2..].find("==") else { break };
            let close = open + 2 + c;
            let r = line_start + open..line_start + close + 2;
            let in_comment = comments.iter().any(|cm| cm.start < r.end && r.start < cm.end);
            if close > open + 2 && !in_comment {
                out.push(r);
            }
            pos = close + 2;
        }
        line_start += line.len();
    }
    out
}

/// 部分复制：只取 `==标记==` 里的文字。同一行的几处用空格连起来，不同行分行
pub fn marked_text(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut last_line: Option<usize> = None;
    for r in find_marks(text) {
        let inner = text[r.start + 2..r.end - 2].trim();
        if inner.is_empty() {
            continue;
        }
        let line_no = text[..r.start].matches('\n').count();
        match lines.last_mut() {
            Some(l) if last_line == Some(line_no) => {
                l.push(' ');
                l.push_str(inner);
            }
            _ => lines.push(inner.to_string()),
        }
        last_line = Some(line_no);
    }
    lines.join("\n")
}

/// 去掉 `%%备注%%` 后的文字（复制内容用）：删完只剩空白的行去掉，行尾空白去掉
pub fn strip_comments(text: &str) -> String {
    let kept: String = split_comments(text)
        .into_iter()
        .filter(|(_, c)| !c)
        .map(|(r, _)| &text[r])
        .collect();
    kept.lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 去掉 #标签 后的正文；多行保持分行，只剩标签的行去掉
pub fn strip_tags(text: &str) -> String {
    drop_words(text, |w| w.len() > 1 && w.starts_with('#') && !w[1..].starts_with('#'))
}

/// 统一条目文字：换行统一成 \n，去掉行尾空白和空行，首行去掉行首空白
/// （之后的行保留行首缩进，比如挂在条目下的子列表）
pub fn clean_text(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i == 0 {
            out.push_str(l.trim_start());
        } else {
            out.push('\n');
            out.push_str(l);
        }
    }
    out
}

/// 把一段文字加到首行末尾（多行条目的标签统一放在首行）
fn push_to_first_line(text: &str, s: &str) -> String {
    let text = text.trim();
    match text.split_once('\n') {
        Some((first, rest)) => {
            let first = first.trim_end();
            if first.is_empty() {
                format!("{s}\n{rest}")
            } else {
                format!("{first} {s}\n{rest}")
            }
        }
        None if text.is_empty() => s.to_string(),
        None => format!("{text} {s}"),
    }
}

/// 缩进宽度，tab 按 4 个空格算
fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// 去掉最多 width 宽度的行首缩进
fn strip_indent(line: &str, width: usize) -> &str {
    let mut w = 0;
    for (i, c) in line.char_indices() {
        if w >= width || !c.is_whitespace() {
            return &line[i..];
        }
        w += if c == '\t' { 4 } else { 1 };
    }
    ""
}

pub fn is_block_id(s: &str) -> bool {
    s.len() > 1
        && s.starts_with('^')
        && s[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 解析一行任务，返回 (前缀, 是否完成, 正文, id)
pub fn parse_task(line: &str) -> Option<(String, bool, String, Option<String>)> {
    let indent_len = line.len() - line.trim_start().len();
    let rest = &line[indent_len..];
    let bullet = rest.chars().next()?;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }
    let rest = rest[1..].strip_prefix(' ')?;
    let (done, after) = if let Some(a) = rest.strip_prefix("[ ]") {
        (false, a)
    } else if let Some(a) = rest.strip_prefix("[x]").or_else(|| rest.strip_prefix("[X]")) {
        (true, a)
    } else {
        return None;
    };
    if !after.is_empty() && !after.starts_with(' ') {
        return None;
    }
    let prefix = format!("{}{} ", &line[..indent_len], bullet);
    let t = after.trim();
    let (text, id) = match t.rsplit_once(' ') {
        Some((head, last)) if is_block_id(last) => {
            (head.trim_end().to_string(), Some(last[1..].to_string()))
        }
        _ if is_block_id(t) => (String::new(), Some(t[1..].to_string())),
        _ => (t.to_string(), None),
    };
    Some((prefix, done, text, id))
}

fn tags_to_text(body: &str, tags: &[String]) -> String {
    if tags.is_empty() {
        return body.trim().to_string();
    }
    let tags: Vec<String> = tags.iter().map(|t| format!("#{t}")).collect();
    push_to_first_line(body, &tags.join(" "))
}

/// 任务行下面的一行是否属于这一条：比任务行缩进更深、不是空行、本身不是任务行
fn is_continuation(line: &str, task_indent: usize) -> bool {
    !line.trim().is_empty() && indent_width(line) > task_indent && parse_task(line).is_none()
}

/// 列出数据目录里的文档。所有文档都放在数据目录这一层，不看子文件夹
fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            continue;
        } else {
            let ext = p
                .extension()
                .map(|x| x.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if matches!(ext.as_str(), "md" | "markdown" | "txt") {
                if let Ok(rel) = p.strip_prefix(root) {
                    out.push(rel.to_path_buf());
                }
            }
        }
    }
}

fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

// ---------- Store ----------

impl Store {
    pub fn open(root: PathBuf) -> Store {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
        let mut s = Store {
            root,
            docs: Vec::new(),
            items: HashMap::new(),
            order: Vec::new(),
            prefs: Prefs::default(),
            error: None,
            trash: Vec::new(),
            rng: seed,
        };
        s.load();
        s
    }

    pub fn reload(&mut self) {
        self.docs.clear();
        self.items.clear();
        self.order.clear();
        self.error = None;
        self.load();
    }

    fn load(&mut self) {
        if let Err(e) = std::fs::create_dir_all(&self.root) {
            self.error = Some(format!("无法创建目录 {}：{e}", self.root.display()));
            return;
        }
        for f in FIXED_DOCS {
            let p = self.root.join(f);
            if !p.exists() {
                let _ = std::fs::write(&p, "");
            }
        }

        let mut files = Vec::new();
        walk(&self.root, &self.root, &mut files);
        files.sort();

        let mut seen: HashSet<String> = HashSet::new();
        let mut file_order: Vec<String> = Vec::new();
        let mut dirty: Vec<usize> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();

        for rel in files {
            let content = match std::fs::read_to_string(self.root.join(&rel)) {
                Ok(c) => c,
                Err(_) => {
                    // 非 UTF-8 文件不处理，避免保存时损坏
                    skipped.push(rel.display().to_string());
                    continue;
                }
            };
            let mut doc = Doc { path: rel.clone(), lines: Vec::new() };
            let mut changed = false;
            let all: Vec<&str> = content.lines().collect();
            let mut li = 0;
            while li < all.len() {
                let line = all[li];
                li += 1;
                match parse_task(line) {
                    Some((prefix, done, mut text, id)) => {
                        // 挂在任务行下面的缩进文字属于这一条
                        let task_indent = indent_width(&prefix);
                        let content_indent = prefix.chars().count();
                        while li < all.len() && is_continuation(all[li], task_indent) {
                            text.push('\n');
                            text.push_str(strip_indent(all[li], content_indent).trim_end());
                            li += 1;
                        }
                        let id = match id {
                            Some(id) if !seen.contains(&id) => id,
                            _ => {
                                changed = true;
                                self.new_id(&seen)
                            }
                        };
                        seen.insert(id.clone());
                        file_order.push(id.clone());
                        self.items.insert(
                            id.clone(),
                            Item { id: id.clone(), prefix, done, text, file: rel.clone() },
                        );
                        doc.lines.push(Line::Item(id));
                    }
                    None => doc.lines.push(Line::Raw(line.to_string())),
                }
            }
            self.docs.push(doc);
            if changed {
                dirty.push(self.docs.len() - 1);
            }
        }

        // 给新条目补上 ^id 并写回
        for i in dirty {
            self.save_doc(i);
        }

        // 读排序和界面偏好
        let state: StateFile = std::fs::read_to_string(self.state_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        self.prefs = state.prefs;
        self.prefs.ui = self.prefs.ui.clone().clamped();
        let mut in_order: HashSet<String> = HashSet::new();
        for id in state.order {
            if self.items.contains_key(&id) && in_order.insert(id.clone()) {
                self.order.push(id);
            }
        }
        for id in file_order {
            if in_order.insert(id.clone()) {
                self.order.push(id);
            }
        }

        if !skipped.is_empty() {
            self.error = Some(format!("以下文件不是 UTF-8 编码，已跳过：{}", skipped.join("、")));
        }
        self.load_trash();
    }

    fn state_path(&self) -> PathBuf {
        self.root.join(STATE_DIR).join(STATE_FILE)
    }

    pub fn save_state(&mut self) {
        let st = StateFile { order: self.order.clone(), prefs: self.prefs.clone() };
        let json = serde_json::to_string_pretty(&st).unwrap_or_default();
        if let Err(e) = write_atomic(&self.state_path(), &json) {
            self.error = Some(format!("保存排序失败：{e}"));
        }
    }

    fn new_id(&mut self, taken: &HashSet<String>) -> String {
        const CH: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        loop {
            let mut s = String::with_capacity(6);
            for _ in 0..6 {
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 7;
                self.rng ^= self.rng << 17;
                s.push(CH[(self.rng % 36) as usize] as char);
            }
            if !taken.contains(&s) && !self.items.contains_key(&s) {
                return s;
            }
        }
    }

    pub fn doc_index(&self, path: &Path) -> Option<usize> {
        self.docs.iter().position(|d| d.path == path)
    }

    /// 某个文件里的条目，按文件中的顺序
    pub fn doc_items(&self, path: &Path) -> Vec<String> {
        let Some(i) = self.doc_index(path) else { return Vec::new() };
        self.docs[i]
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Item(id) => Some(id.clone()),
                Line::Raw(_) => None,
            })
            .collect()
    }

    fn save_doc(&mut self, i: usize) {
        let doc = &self.docs[i];
        let mut out = String::new();
        for l in &doc.lines {
            match l {
                Line::Raw(s) => out.push_str(s),
                Line::Item(id) => {
                    if let Some(it) = self.items.get(id) {
                        out.push_str(&it.to_line());
                    }
                }
            }
            out.push('\n');
        }
        let path = self.root.join(&doc.path);
        if let Err(e) = write_atomic(&path, &out) {
            self.error = Some(format!("保存 {} 失败：{e}", doc.path.display()));
        }
    }

    fn save_file_of(&mut self, id: &str) {
        let Some(file) = self.items.get(id).map(|it| it.file.clone()) else { return };
        if let Some(i) = self.doc_index(&file) {
            self.save_doc(i);
        }
    }

    fn ensure_doc(&mut self, rel: &Path) -> usize {
        if let Some(i) = self.doc_index(rel) {
            return i;
        }
        self.docs.push(Doc { path: rel.to_path_buf(), lines: Vec::new() });
        self.docs.sort_by(|a, b| a.path.cmp(&b.path));
        self.doc_index(rel).unwrap()
    }

    // ---------- 操作 ----------

    pub fn add(&mut self, text: &str, file: &Path) -> String {
        let id = self.new_id(&HashSet::new());
        let item = Item {
            id: id.clone(),
            prefix: "- ".into(),
            done: false,
            text: clean_text(text),
            file: file.to_path_buf(),
        };
        self.items.insert(id.clone(), item);
        let i = self.ensure_doc(file);
        // 追加到文件末尾，去掉末尾多余空行
        while matches!(self.docs[i].lines.last(), Some(Line::Raw(s)) if s.trim().is_empty()) {
            self.docs[i].lines.pop();
        }
        self.docs[i].lines.push(Line::Item(id.clone()));
        self.save_doc(i);
        self.order.push(id.clone());
        self.save_state();
        id
    }

    pub fn set_done(&mut self, id: &str, done: bool) {
        if let Some(it) = self.items.get_mut(id) {
            if it.done != done {
                it.done = done;
                self.save_file_of(id);
            }
        }
    }

    pub fn set_text(&mut self, id: &str, text: &str) {
        if let Some(it) = self.items.get_mut(id) {
            it.text = clean_text(text);
            self.save_file_of(id);
        }
    }

    pub fn delete(&mut self, id: &str) {
        let Some(it) = self.items.get(id).cloned() else { return };
        if let Some(i) = self.doc_index(&it.file) {
            self.docs[i].lines.retain(|l| !matches!(l, Line::Item(x) if x == id));
            self.save_doc(i);
        }
        self.items.remove(id);
        self.order.retain(|x| x != id);
        self.save_state();
    }

    /// 移到另一个文件（剪切到目标文件末尾）
    pub fn move_to(&mut self, id: &str, file: &Path) {
        let Some(old) = self.items.get(id).map(|it| it.file.clone()) else { return };
        if old == file {
            return;
        }
        if let Some(i) = self.doc_index(&old) {
            self.docs[i].lines.retain(|l| !matches!(l, Line::Item(x) if x == id));
            self.save_doc(i);
        }
        if let Some(it) = self.items.get_mut(id) {
            it.file = file.to_path_buf();
        }
        let i = self.ensure_doc(file);
        while matches!(self.docs[i].lines.last(), Some(Line::Raw(s)) if s.trim().is_empty()) {
            self.docs[i].lines.pop();
        }
        self.docs[i].lines.push(Line::Item(id.to_string()));
        self.save_doc(i);
    }

    /// 追加标签；replace 为 true 时替换掉原有全部标签
    pub fn add_tag(&mut self, id: &str, tag: &str, replace: bool) {
        let Some(it) = self.items.get_mut(id) else { return };
        let mut tags = if replace { Vec::new() } else { it.tags() };
        if !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_string());
        }
        it.text = tags_to_text(&it.body(), &tags);
        self.save_file_of(id);
    }

    /// 切换某条的一个标签：有就去掉，没有就加到首行末尾
    pub fn toggle_item_tag(&mut self, id: &str, tag: &str) {
        let Some(text) = self.items.get(id).map(|it| it.text.clone()) else { return };
        self.set_text(id, &toggle_tag(&text, tag));
    }

    /// 改主标签（第一个标签）；None 表示清空所有标签。
    /// 主标签已经是 tag 或它的子标签（#资料/AI 之于 资料）时不改，免得子标签被换成父标签。
    pub fn set_primary_tag(&mut self, id: &str, tag: Option<&str>) {
        let Some(it) = self.items.get_mut(id) else { return };
        let mut tags = it.tags();
        if let (Some(t), Some(first)) = (tag, tags.first()) {
            if first == t || first.starts_with(&format!("{t}/")) {
                return;
            }
        }
        match tag {
            None => tags.clear(),
            Some(t) => {
                tags.retain(|x| x != t);
                if tags.is_empty() {
                    tags.push(t.to_string());
                } else {
                    tags[0] = t.to_string();
                }
                tags.dedup();
            }
        }
        it.text = tags_to_text(&it.body(), &tags);
        self.save_file_of(id);
    }

    /// 待办区排序：把 id 放到 target 的前面或后面
    pub fn reorder(&mut self, id: &str, target: &str, after: bool) {
        if id == target {
            return;
        }
        self.order.retain(|x| x != id);
        let pos = self.order.iter().position(|x| x == target).unwrap_or(self.order.len());
        let pos = if after { (pos + 1).min(self.order.len()) } else { pos };
        self.order.insert(pos, id.to_string());
        self.save_state();
    }

    /// 新建文档，name 不带扩展名时补 .md
    pub fn create_doc(&mut self, name: &str) -> Option<PathBuf> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if name.contains(['/', '\\']) || name.starts_with('.') {
            self.error = Some(format!("文件名不能包含 / 或 \\，也不能以 . 开头：{name}"));
            return None;
        }
        let lower = name.to_lowercase();
        let file = if lower.ends_with(".md") || lower.ends_with(".txt") || lower.ends_with(".markdown") {
            name.to_string()
        } else {
            format!("{name}.md")
        };
        let rel = PathBuf::from(file);
        let abs = self.root.join(&rel);
        if !abs.exists() {
            if let Err(e) = write_atomic(&abs, "") {
                self.error = Some(format!("新建失败：{e}"));
                return None;
            }
        }
        self.ensure_doc(&rel);
        Some(rel)
    }

    /// 导入外部文件：复制进数据目录，原文件不动。不 reload，导入完一批后由调用方 reload。
    /// - 只收 .md / .txt / .markdown，且必须是 UTF-8；
    /// - 本来就在数据目录这一层的文件不复制，直接返回它的位置；
    /// - 同名时改叫「名字 2.md」「名字 3.md」……；
    /// - 和现有条目重复的 ^id 去掉（读取时会重新编号），免得和原来的条目混在一起。
    pub fn import_file(&mut self, src: &Path) -> Result<PathBuf, String> {
        let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !matches!(ext.as_str(), "md" | "txt" | "markdown") {
            return Err(format!("{name}（只能导入 .md / .txt）"));
        }
        if let (Ok(s), Ok(r)) = (src.canonicalize(), self.root.canonicalize()) {
            if s.parent() == Some(r.as_path()) {
                return Ok(PathBuf::from(&name));
            }
        }
        let text = match std::fs::read(src) {
            Ok(b) => match String::from_utf8(b) {
                Ok(t) => t,
                Err(_) => return Err(format!("{name}（不是 UTF-8 编码）")),
            },
            Err(e) => return Err(format!("{name}（{e}）")),
        };
        // 去掉和现有条目重复的 ^id
        let text: String = text
            .split_inclusive('\n')
            .map(|raw| {
                let line = raw.trim_end_matches(['\n', '\r']);
                let eol = &raw[line.len()..];
                match parse_task(line) {
                    Some((_, _, _, Some(id))) if self.items.contains_key(&id) => {
                        let t = line.trim_end();
                        format!("{}{eol}", t[..t.len() - id.len() - 1].trim_end())
                    }
                    _ => raw.to_string(),
                }
            })
            .collect();
        let stem = src.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let dot_ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut rel = PathBuf::from(&name);
        let mut n = 2;
        while self.root.join(&rel).exists() {
            rel = PathBuf::from(format!("{stem} {n}{dot_ext}"));
            n += 1;
        }
        let to = self.root.join(&rel);
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        write_atomic(&to, &text).map_err(|e| format!("{name}（{e}）"))?;
        Ok(rel)
    }

    /// 生成使用说明文档。已有同名文件时不覆盖，改叫「使用说明 2.md」「使用说明 3.md」……
    /// 返回生成的文档路径
    pub fn install_guide(&mut self) -> Option<PathBuf> {
        let mut rel = PathBuf::from(GUIDE);
        let mut n = 2;
        while self.root.join(&rel).exists() {
            rel = PathBuf::from(format!("使用说明 {n}.md"));
            n += 1;
        }
        if let Err(e) = write_atomic(&self.root.join(&rel), GUIDE_TEXT) {
            self.error = Some(format!("生成使用说明失败：{e}"));
            return None;
        }
        // 重新读取：条目补上 ^id、进入列表
        self.reload();
        Some(rel)
    }

    /// 重命名文档，留在原目录；name 不带扩展名时沿用原扩展名。成功返回新路径。
    pub fn rename_doc(&mut self, rel: &Path, name: &str) -> Option<PathBuf> {
        if is_fixed_doc(rel) {
            self.error = Some(format!("{} 是固定文档，不能改名", rel.display()));
            return None;
        }
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        if name.contains(['/', '\\']) || name.starts_with('.') {
            self.error = Some(format!("文件名不能包含 / 或 \\，也不能以 . 开头：{name}"));
            return None;
        }
        let lower = name.to_lowercase();
        let file = if lower.ends_with(".md") || lower.ends_with(".txt") || lower.ends_with(".markdown") {
            name.to_string()
        } else {
            match rel.extension() {
                Some(ext) => format!("{name}.{}", ext.to_string_lossy()),
                None => format!("{name}.md"),
            }
        };
        let new_rel = rel.with_file_name(&file);
        if new_rel == rel {
            return Some(new_rel);
        }
        let from = self.root.join(rel);
        let to = self.root.join(&new_rel);
        // 只改大小写时（Windows / macOS 不区分大小写），目标“已存在”的其实是自己
        let same_ignoring_case =
            new_rel.to_string_lossy().to_lowercase() == rel.to_string_lossy().to_lowercase();
        if to.exists() && !same_ignoring_case {
            self.error = Some(format!("{} 已存在，没有改名", new_rel.display()));
            return None;
        }
        if let Err(e) = std::fs::rename(&from, &to) {
            self.error = Some(format!("改名失败：{e}"));
            return None;
        }
        if let Some(i) = self.doc_index(rel) {
            self.docs[i].path = new_rel.clone();
        }
        for it in self.items.values_mut() {
            if it.file == rel {
                it.file = new_rel.clone();
            }
        }
        self.docs.sort_by(|a, b| a.path.cmp(&b.path));
        if !self.in_todo(rel) {
            self.set_in_todo(rel, true);
            self.set_in_todo(&new_rel, false);
        }
        Some(new_rel)
    }

    // ---------- TODO（右栏）----------

    /// 这个文档的条目是否出现在右栏 TODO 里
    pub fn in_todo(&self, rel: &Path) -> bool {
        let key = rel.to_string_lossy().replace('\\', "/");
        !self.prefs.not_in_todo.contains(&key)
    }

    /// 设置文档是否放入 TODO，并保存
    pub fn set_in_todo(&mut self, rel: &Path, on: bool) {
        let key = rel.to_string_lossy().replace('\\', "/");
        self.prefs.not_in_todo.retain(|k| *k != key);
        if !on {
            self.prefs.not_in_todo.push(key);
        }
        self.save_state();
    }

    // ---------- 回收站 ----------

    fn trash_dir(&self) -> PathBuf {
        self.root.join(STATE_DIR).join(TRASH_DIR)
    }

    /// 读回收站索引；文件已经不在的条目丢掉
    fn load_trash(&mut self) {
        let dir = self.trash_dir();
        let list: Vec<TrashEntry> = std::fs::read_to_string(dir.join(TRASH_INDEX))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        self.trash = list.into_iter().filter(|e| dir.join(&e.file).is_file()).collect();
        self.trash.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
    }

    fn save_trash(&mut self) {
        let json = serde_json::to_string_pretty(&self.trash).unwrap_or_default();
        if let Err(e) = write_atomic(&self.trash_dir().join(TRASH_INDEX), &json) {
            self.error = Some(format!("保存回收站失败：{e}"));
        }
    }

    /// 某个文档里有几条素材
    pub fn doc_item_count(&self, rel: &Path) -> usize {
        self.doc_items(rel).len()
    }

    /// 删除文档：整个文件移进回收站，它的条目从列表里去掉。成功返回 true
    pub fn delete_doc(&mut self, rel: &Path) -> bool {
        if is_fixed_doc(rel) {
            self.error = Some(format!("{} 是固定文档，不能删除", rel.display()));
            return false;
        }
        let src = self.root.join(rel);
        let dir = self.trash_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error = Some(format!("无法创建回收站：{e}"));
            return false;
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let name = rel.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut file = format!("{}_{name}", now.as_millis());
        let mut n = 1;
        while dir.join(&file).exists() {
            file = format!("{}_{n}_{name}", now.as_millis());
            n += 1;
        }
        if src.exists() {
            if let Err(e) = std::fs::rename(&src, dir.join(&file)) {
                self.error = Some(format!("删除 {} 失败：{e}", rel.display()));
                return false;
            }
        }
        let items = self.doc_item_count(rel);
        self.docs.retain(|d| d.path != rel);
        let gone: HashSet<String> =
            self.items.values().filter(|it| it.file == rel).map(|it| it.id.clone()).collect();
        self.items.retain(|id, _| !gone.contains(id));
        self.order.retain(|id| !gone.contains(id));
        self.save_state();
        self.trash.insert(
            0,
            TrashEntry {
                file,
                original: rel.to_string_lossy().replace('\\', "/"),
                deleted_at: now.as_secs(),
                items,
            },
        );
        self.save_trash();
        true
    }

    /// 从回收站还原到原位置；原位置已有同名文件时改名为「名字（恢复）.md」。返回还原后的路径
    pub fn restore_trash(&mut self, file: &str) -> Option<PathBuf> {
        let i = self.trash.iter().position(|e| e.file == file)?;
        let entry = self.trash[i].clone();
        // 只用文件名：都还原到数据目录这一层
        let orig = PathBuf::from(
            Path::new(&entry.original).file_name().map(|n| n.to_os_string()).unwrap_or_default(),
        );
        let stem = orig.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = orig.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut rel = orig.clone();
        let mut n = 1;
        while self.root.join(&rel).exists() {
            let suffix = if n == 1 { "（恢复）".to_string() } else { format!("（恢复{n}）") };
            rel = orig.with_file_name(format!("{stem}{suffix}{ext}"));
            n += 1;
        }
        let to = self.root.join(&rel);
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::rename(self.trash_dir().join(&entry.file), &to) {
            self.error = Some(format!("还原失败：{e}"));
            return None;
        }
        self.trash.remove(i);
        self.save_trash();
        // 重新读取：还原的文档和它的条目回到列表里
        self.reload();
        Some(rel)
    }

    /// 彻底删除回收站里的文档（不可恢复）
    pub fn purge_trash(&mut self, files: &[String]) {
        let dir = self.trash_dir();
        for f in files {
            match std::fs::remove_file(dir.join(f)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    self.error = Some(format!("彻底删除 {f} 失败：{e}"));
                    continue;
                }
            }
            self.trash.retain(|e| &e.file != f);
        }
        self.save_trash();
    }

    pub fn read_raw(&self, rel: &Path) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap_or_default()
    }

    /// 整篇原文保存后重新加载全部数据
    pub fn write_raw(&mut self, rel: &Path, content: &str) {
        match write_atomic(&self.root.join(rel), content) {
            Ok(()) => self.reload(),
            Err(e) => self.error = Some(format!("保存 {} 失败：{e}", rel.display())),
        }
    }

    /// 所有标签（含父级），按名称排序
    pub fn all_tags(&self) -> Vec<String> {
        let mut set: Vec<String> = Vec::new();
        for it in self.items.values() {
            for t in it.tags() {
                let parts: Vec<&str> = t.split('/').collect();
                for n in 1..=parts.len() {
                    let p = parts[..n].join("/");
                    if !set.contains(&p) {
                        set.push(p);
                    }
                }
            }
        }
        set.sort();
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let (p, d, t, id) = parse_task("- [ ] 写脚本 #短视频 ^abc123").unwrap();
        assert_eq!(p, "- ");
        assert!(!d);
        assert_eq!(t, "写脚本 #短视频");
        assert_eq!(id.as_deref(), Some("abc123"));

        let (p, d, t, id) = parse_task("  * [x] 已用素材").unwrap();
        assert_eq!(p, "  * ");
        assert!(d);
        assert_eq!(t, "已用素材");
        assert!(id.is_none());

        assert!(parse_task("- 普通列表").is_none());
        assert!(parse_task("# 标题").is_none());
        assert!(parse_task("-[ ] 缺空格").is_none());
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("sucard-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn multiline_items() {
        let root = temp_root("multi");
        let src = "# 歌词\n\
                   - [ ] 第一首 #歌 ^aaa111\n  男：第一句\n  女：第二句\n\
                   \n\
                   段落不属于任何条目\n\
                   - [x] 第二首 ^bbb222\n  - 子列表也归这一条\n    - 更深一层\n  - [ ] 子任务是独立条目 ^ccc333\n";
        std::fs::write(root.join("a.md"), src).unwrap();
        let mut s = Store::open(root.clone());

        assert_eq!(s.items["aaa111"].text, "第一首 #歌\n男：第一句\n女：第二句");
        assert_eq!(s.items["aaa111"].body(), "第一首\n男：第一句\n女：第二句");
        assert_eq!(s.items["bbb222"].text, "第二首\n- 子列表也归这一条\n  - 更深一层");
        assert_eq!(s.items["ccc333"].text, "子任务是独立条目");

        // 不改内容时写回，文件原样不变
        s.set_done("aaa111", true);
        s.set_done("aaa111", false);
        assert_eq!(std::fs::read_to_string(root.join("a.md")).unwrap(), src);

        // 粘贴进来的 \r\n、空行、行尾空白被整理；标签留在首行
        s.set_text("aaa111", "第一首 #歌\r\n\r\n男：改过的一句  \r\n");
        s.add_tag("aaa111", "副歌", false);
        let out = std::fs::read_to_string(root.join("a.md")).unwrap();
        assert!(out.contains("- [ ] 第一首 #歌 #副歌 ^aaa111\n  男：改过的一句\n\n段落不属于任何条目\n"), "{out}");

        // 重新读取后一致
        s.reload();
        assert_eq!(s.items["aaa111"].text, "第一首 #歌 #副歌\n男：改过的一句");

        // 切换标签：去掉、加上，多行不受影响
        s.toggle_item_tag("aaa111", "歌");
        assert_eq!(s.items["aaa111"].text, "第一首 #副歌\n男：改过的一句");
        s.toggle_item_tag("aaa111", "新歌");
        assert_eq!(s.items["aaa111"].text, "第一首 #副歌 #新歌\n男：改过的一句");

        // 改主标签：已是它或它的子标签就不动；名字相近的不算子标签
        s.set_text("aaa111", "第一首 #资料/AI #歌");
        s.set_primary_tag("aaa111", Some("资料"));
        assert_eq!(s.items["aaa111"].text, "第一首 #资料/AI #歌");
        s.set_primary_tag("aaa111", Some("资"));
        assert_eq!(s.items["aaa111"].text, "第一首 #资 #歌");
        s.set_primary_tag("aaa111", Some("歌"));
        assert_eq!(s.items["aaa111"].text, "第一首 #歌");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ui_prefs_defaults_and_clamp() {
        // 旧版 state.json 没有 ui 字段；只写了一部分字段的也能读
        let p: Prefs = serde_json::from_str(r#"{"hide_done":true}"#).unwrap();
        assert!(p.hide_done);
        assert_eq!(p.ui, UiPrefs::default());
        let p: Prefs = serde_json::from_str(r#"{"ui":{"gap":10}}"#).unwrap();
        assert_eq!(p.ui.gap, 10.0);
        assert_eq!(p.ui.radius, 6.0);
        // 离谱的值会被限制住
        let u = UiPrefs { theme: 9, font_size: 99.0, gap: -3.0, radius: f32::NAN, nav_rows: 0.0, small_pct: 10.0, ..Default::default() }.clamped();
        assert_eq!((u.theme, u.font_size, u.gap, u.radius, u.nav_rows, u.small_pct), (9, 22.0, 0.0, 6.0, 1.0, 60.0));
        // 已移除的「护眼（浅）」(4) 读到时换成樱米 (5)
        assert_eq!(UiPrefs { theme: 4, ..Default::default() }.clamped().theme, 5);
    }

    #[test]
    fn multiline_tag_ops() {
        assert_eq!(strip_tags("标题 #a\n  - 子项 #b\n#c"), "标题\n  - 子项");
        assert_eq!(tags_to_text("标题\n第二行", &["a".into()]), "标题 #a\n第二行");
        assert_eq!(ensure_tag("标题\n第二行", "a"), "标题 #a\n第二行");
        assert_eq!(toggle_tag("标题 #a\n第二行", "a"), "标题\n第二行");
        assert_eq!(clean_text("  首行  \r\n\r\n  缩进行 \n"), "首行\n  缩进行");
    }

    #[test]
    fn trash_delete_restore_purge() {
        let root = temp_root("trash");
        let a = "# 标题\n- [ ] 一 ^aaa111\n- [ ] 二 ^bbb222\n";
        std::fs::write(root.join("a.md"), a).unwrap();
        std::fs::write(root.join("b.md"), "- [ ] 三 ^ccc333\n").unwrap();
        let mut s = Store::open(root.clone());
        let rel = PathBuf::from("a.md");

        // 删除：文件进回收站，条目消失，回收站记下原位置和条数
        assert!(s.delete_doc(&rel));
        assert!(!root.join(&rel).exists());
        assert!(s.doc_index(&rel).is_none());
        assert!(!s.items.contains_key("aaa111") && !s.order.contains(&"aaa111".to_string()));
        assert!(s.items.contains_key("ccc333"));
        assert_eq!(s.trash.len(), 1);
        assert_eq!((s.trash[0].original.as_str(), s.trash[0].items), ("a.md", 2));
        let in_trash = root.join(".sucard/回收站").join(&s.trash[0].file);
        assert_eq!(std::fs::read_to_string(&in_trash).unwrap(), a);

        // 重新打开后回收站还在；回收站里的文档不会被当成普通文档扫描
        let mut s = Store::open(root.clone());
        assert_eq!(s.trash.len(), 1);
        assert!(!s.items.contains_key("aaa111"));

        // 还原：回到原位置，条目回来
        let f = s.trash[0].file.clone();
        assert_eq!(s.restore_trash(&f), Some(rel.clone()));
        assert_eq!(std::fs::read_to_string(root.join(&rel)).unwrap(), a);
        assert!(s.items.contains_key("aaa111") && s.trash.is_empty());

        // 原位置已有同名文件：还原成「a（恢复）.md」
        assert!(s.delete_doc(&rel));
        std::fs::write(root.join(&rel), "").unwrap();
        let f = s.trash[0].file.clone();
        assert_eq!(s.restore_trash(&f), Some(PathBuf::from("a（恢复）.md")));

        // 彻底删除（批量）：文件没了，索引也没了
        assert!(s.delete_doc(&PathBuf::from("b.md")));
        assert!(s.delete_doc(&PathBuf::from("a（恢复）.md")));
        assert_eq!(s.trash.len(), 2);
        let all: Vec<String> = s.trash.iter().map(|e| e.file.clone()).collect();
        s.purge_trash(&all);
        assert!(s.trash.is_empty());
        assert_eq!(std::fs::read_dir(root.join(".sucard/回收站")).unwrap().count(), 1, "只剩 index.json");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fixed_docs_cannot_be_deleted_or_renamed() {
        let root = temp_root("fixed");
        let mut s = Store::open(root.clone());
        // 启动时自动建好
        for f in FIXED_DOCS {
            assert!(root.join(f).is_file(), "{f} 没建");
            assert!(s.doc_index(Path::new(f)).is_some());
        }
        assert!(!s.delete_doc(Path::new(VAULT)));
        assert!(root.join(VAULT).is_file() && s.trash.is_empty());
        assert!(s.rename_doc(Path::new(INBOX), "别的").is_none());
        assert!(root.join(INBOX).is_file());
        // 子文件夹不扫描：里面的文档不出现，旧回收站里子文件夹的文档还原到数据目录这一层
        std::fs::create_dir_all(root.join("子")).unwrap();
        std::fs::write(root.join("子/x.md"), "- [ ] 子文件夹里的 ^sub111\n").unwrap();
        s.reload();
        assert!(!s.items.contains_key("sub111"));
        assert!(s.docs.iter().all(|d| d.path.parent() == Some(Path::new(""))));
        std::fs::write(root.join("y.md"), "").unwrap();
        s.reload();
        assert!(s.delete_doc(Path::new("y.md")));
        s.trash[0].original = "旧/y.md".into();
        let f = s.trash[0].file.clone();
        assert_eq!(s.restore_trash(&f), Some(PathBuf::from("y.md")));
        assert!(s.create_doc("a/b").is_none(), "名字不能带路径");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tags_in_comments_dont_count() {
        let text = "正文 #真 %%备注 #假 继续\n  第二行 #也假%% #真2 %%#假3%%";
        assert_eq!(parse_tags(text), vec!["真", "真2"]);
        // 正文里去掉标签时，备注里的 #假 原样保留
        assert_eq!(strip_tags(text), "正文 %%备注 #假 继续\n  第二行 #也假%% %%#假3%%");
        // 切换标签：备注里有 #假 不算已有，会加到首行；去掉 #真 不碰备注
        assert!(toggle_tag(text, "假").lines().next().unwrap().trim_end().ends_with("#假"));
        assert!(!toggle_tag(text, "真").contains("#真 "));
        assert!(toggle_tag(text, "真").contains("#假"));
        // 复制内容：标签和备注都不要
        assert_eq!(strip_comments(&strip_tags(text)), "正文");
    }

    #[test]
    fn urls() {
        let f = |t: &str| find_urls(t).into_iter().map(|r| t[r].to_string()).collect::<Vec<_>>();
        assert_eq!(f("看这个https://a.com/x?y=1，很好"), vec!["https://a.com/x?y=1"]);
        assert_eq!(f("见 www.abc.com. 和 http://b.cn/p(1) 结束"), vec!["www.abc.com", "http://b.cn/p(1)"]);
        assert_eq!(f("（https://c.com/d）"), vec!["https://c.com/d"]);
        assert_eq!(f("(see https://c.com/d)"), vec!["https://c.com/d"]);
        assert_eq!(f("%%出处 https://e.com/f%%"), vec!["https://e.com/f"]);
        assert_eq!(f("abcwww.x.com 和 https:// 和 没有"), Vec::<String>::new());
    }

    #[test]
    fn marks() {
        let t = "标题 ==A 部分== 中间 ==B== 尾\n  ==第二行== 普通 == 落单\n  %%==备注里==%%\n====";
        assert_eq!(marked_text(t), "A 部分 B\n第二行");
        assert_eq!(find_marks(t).len(), 3);
        assert_eq!(marked_text("没有标记"), "");
    }

    #[test]
    fn guide_doc() {
        let root = temp_root("guide");
        let mut s = Store::open(root.clone());
        let rel = s.install_guide().unwrap();
        assert_eq!(rel, PathBuf::from(GUIDE));
        let items = s.doc_items(&rel);
        let tasks = GUIDE_TEXT.lines().filter(|l| l.starts_with("- [")).count();
        assert_eq!(items.len(), tasks, "每个任务行都应是一条素材，缩进的内容不能变成单独的条目");
        // 说明文字里不能冒出意外的标签：只有「说明」和它的子标签
        for id in &items {
            for t in s.items[id].tags() {
                assert!(t == "说明" || t.starts_with("说明/"), "意外的标签 {t}：{}", s.items[id].text);
            }
        }
        // 示例条目：复制全部跳过备注，部分复制只取标记
        let find = |s: &Store, k: &str| s.items.values().find(|it| it.text.contains(k)).unwrap().body();
        assert_eq!(strip_comments(&find(&s, "这一句会被复制")).lines().next(), Some("示例：这一句会被复制"));
        assert_eq!(marked_text(&find(&s, "只复制这一段")), "只复制这一段 还有这一段");
        assert!(s.items.values().any(|it| it.done), "应有一条已完成的示例");
        // 已经有了就不覆盖
        let again = s.install_guide().unwrap();
        assert_eq!(again, PathBuf::from("使用说明 2.md"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn import_files() {
        let root = temp_root("import");
        let outside = temp_root("import-src");
        std::fs::write(root.join("笔记.md"), "- [ ] 原来的 ^aaa111\n").unwrap();
        let mut s = Store::open(root.clone());
        std::fs::write(outside.join("笔记.md"), "# 外面\n- [ ] 重复编号 ^aaa111\n- [ ] 新的 ^bbb222\n").unwrap();
        std::fs::write(outside.join("图.png"), [0u8, 1, 2]).unwrap();
        std::fs::write(outside.join("乱码.txt"), [0xffu8, 0xfe, 0x00]).unwrap();

        // 同名改名；重复的 ^id 去掉，其余原样
        let rel = s.import_file(&outside.join("笔记.md")).unwrap();
        assert_eq!(rel, PathBuf::from("笔记 2.md"));
        assert_eq!(
            std::fs::read_to_string(root.join(&rel)).unwrap(),
            "# 外面\n- [ ] 重复编号\n- [ ] 新的 ^bbb222\n"
        );
        assert_eq!(std::fs::read_to_string(outside.join("笔记.md")).unwrap().lines().count(), 3, "原文件不能动");
        // 再导入一次：笔记 3.md
        assert_eq!(s.import_file(&outside.join("笔记.md")).unwrap(), PathBuf::from("笔记 3.md"));
        // 不收的文件
        assert!(s.import_file(&outside.join("图.png")).unwrap_err().contains("图.png"));
        assert!(s.import_file(&outside.join("乱码.txt")).unwrap_err().contains("UTF-8"));
        // 已经在数据目录里的：不复制
        assert_eq!(s.import_file(&root.join("笔记.md")).unwrap(), PathBuf::from("笔记.md"));
        s.reload();
        assert_eq!(s.items["aaa111"].text, "原来的", "原来的条目编号不变");
        assert!(s.items.values().any(|it| it.text == "重复编号"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn not_in_todo() {
        let root = temp_root("notodo");
        std::fs::write(root.join("a.md"), "- [ ] 一 ^aaa111\n").unwrap();
        let mut s = Store::open(root.clone());
        let a = PathBuf::from("a.md");
        assert!(s.in_todo(&a));
        s.set_in_todo(&a, false);
        assert!(!s.in_todo(&a));
        // 保存了；改名后跟着走
        let s2 = Store::open(root.clone());
        assert!(!s2.in_todo(&a));
        let b = s.rename_doc(&a, "b").unwrap();
        assert!(!s.in_todo(&b) && s.in_todo(&a));
        s.set_in_todo(&b, true);
        assert!(s.in_todo(&b) && s.prefs.not_in_todo.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn comments() {
        assert_eq!(strip_comments("正文 %%备注%% 后面"), "正文  后面");
        assert_eq!(strip_comments("正文\n%%整行备注%%\n第三行"), "正文\n第三行");
        assert_eq!(strip_comments("正文 %%跨\n行备注%%\n下一行"), "正文\n下一行");
        assert_eq!(strip_comments("正文 %%没有收尾\n都算备注"), "正文");
        assert_eq!(strip_comments("没有备注"), "没有备注");
        assert_eq!(strip_comments("%%全是备注%%"), "");
        let segs = split_comments("a%%b%%c");
        assert_eq!(segs, vec![(0..1, false), (1..6, true), (6..7, false)]);
    }

    #[test]
    fn rename_doc() {
        let root = std::env::temp_dir().join(format!("sucard-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("旧.md"), "# 标题\n- [ ] 一条 #灵感 ^aaa111\n").unwrap();
        std::fs::write(root.join("占用.md"), "").unwrap();
        let mut s = Store::open(root.clone());
        let old = PathBuf::from("旧.md");

        // 不带扩展名：沿用 .md，留在原目录，条目跟着走
        let new = s.rename_doc(&old, " 新名字 ").unwrap();
        assert_eq!(new, PathBuf::from("新名字.md"));
        assert!(root.join(&new).exists() && !root.join(&old).exists());
        assert_eq!(s.items["aaa111"].file, new);
        assert!(s.doc_index(&new).is_some() && s.doc_index(&old).is_none());

        // 目标已存在、名字非法：不改，给出提示
        assert!(s.rename_doc(&new, "占用").is_none());
        assert!(s.error.take().is_some());
        assert!(s.rename_doc(&new, "a/b").is_none());
        assert!(root.join(&new).exists());

        // 只改大小写
        let up = s.rename_doc(&new, "ABC").unwrap();
        let low = s.rename_doc(&up, "abc").unwrap();
        assert_eq!(low, PathBuf::from("abc.md"));
        assert_eq!(s.items["aaa111"].file, low);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tags() {
        assert_eq!(parse_tags("测评 #资料/AI #灵感，"), vec!["资料/AI", "灵感"]);
        assert_eq!(strip_tags("测评 #资料/AI 链接"), "测评 链接");
        assert_eq!(tags_to_text("测评", &["a".into(), "b".into()]), "测评 #a #b");

        // 自动补标签：没有才加；已有它或它的子标签就不动；名字相近的不算
        assert_eq!(ensure_tag("写脚本 ", "灵感"), "写脚本 #灵感");
        assert_eq!(ensure_tag("写脚本 #灵感", "灵感"), "写脚本 #灵感");
        assert_eq!(ensure_tag("测评 #资料/AI", "资料"), "测评 #资料/AI");
        assert_eq!(ensure_tag("测评 #资料库", "资料"), "测评 #资料库 #资料");

        // 切换标签：加上、去掉，父子标签互不影响
        assert_eq!(toggle_tag("", "灵感"), "#灵感 ");
        assert_eq!(toggle_tag("写脚本  ", "灵感"), "写脚本 #灵感 ");
        assert_eq!(toggle_tag("写脚本 #灵感 ", "资料/AI"), "写脚本 #灵感 #资料/AI ");
        assert_eq!(toggle_tag("写脚本 #灵感， 结尾", "灵感"), "写脚本 结尾 ");
        assert_eq!(toggle_tag("#资料/AI", "资料"), "#资料/AI #资料 ");
        assert_eq!(toggle_tag("#资料/AI #资料", "资料"), "#资料/AI ");
    }
}
