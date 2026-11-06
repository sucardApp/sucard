#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod store;

use eframe::egui::{
    self, Color32, CursorIcon, FontFamily, Id, Key, Label, Rect, RichText, ScrollArea, Sense,
    Stroke, TextEdit, Ui,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::{find_marks, find_urls, is_block_id, marked_text, is_fixed_doc, parse_task, split_comments, strip_comments, tag_of_word, Item, Store, UiPrefs, FIXED_DOCS, INBOX};

/// 默认数据目录的文件夹名（建在用户目录下）
const DEFAULT_DIR: &str = "Sucard";

/// 默认数据目录：用户目录下的 Sucard 文件夹。
/// Windows 先看 USERPROFILE：Git、MSYS2 等工具会在自己的环境里设置 HOME，
/// 如果先看 HOME，从那些窗口启动时素材库会建到别处。其他系统用 HOME。
/// get 用来读环境变量（测试时传入假的）。
fn default_root(get: impl Fn(&str) -> Option<std::ffi::OsString>, windows: bool) -> PathBuf {
    let (first, second) = if windows { ("USERPROFILE", "HOME") } else { ("HOME", "USERPROFILE") };
    get(first)
        .filter(|v| !v.is_empty())
        .or_else(|| get(second).filter(|v| !v.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_DIR)
}

fn main() -> eframe::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SUCARD_DIR").map(PathBuf::from))
        .unwrap_or_else(|| default_root(|k| std::env::var_os(k), cfg!(windows)));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([760.0, 420.0])
            // 窗口和任务栏图标（C 方案：叠卡）
            .with_icon(std::sync::Arc::new(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")).unwrap_or_default(),
            ))
            .with_title("素签"),
        ..Default::default()
    };
    eframe::run_native(
        "素签",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, root)))),
    )
}

// ---------- 界面外观设置 ----------

/// 强调色预设：(名字, 基色)；None 表示 egui 默认的蓝色
const ACCENTS: [(&str, Option<(u8, u8, u8)>); 7] = [
    ("默认蓝", None),
    ("绿", Some((34, 160, 90))),
    ("紫", Some((124, 92, 214))),
    ("橙", Some((230, 126, 34))),
    ("粉", Some((214, 82, 140))),
    ("灰", Some((110, 110, 120))),
    // #015697
    ("深蓝", Some((1, 86, 151))),
];

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// 设置面板色块上显示的颜色
fn accent_swatch(i: usize) -> Color32 {
    match ACCENTS.get(i).and_then(|a| a.1) {
        Some((r, g, b)) => Color32::from_rgb(r, g, b),
        None => Color32::from_rgb(0, 120, 215),
    }
}

/// 把外观设置应用到 egui（浅色、深色两套样式都设好，主题切换时不用再算）。
/// 只在启动和设置改变时调用一次，不是每帧。
fn apply_ui_prefs(ctx: &egui::Context, p: &UiPrefs) {
    let palette = warm_of_theme(p.theme);
    ctx.set_theme(match (p.theme, palette) {
        // 配色主题放在浅色或深色样式里
        (_, Some(c)) if c.dark => egui::ThemePreference::Dark,
        (1, _) | (_, Some(_)) => egui::ThemePreference::Light,
        (2, _) => egui::ThemePreference::Dark,
        _ => egui::ThemePreference::System,
    });
    let scale = p.font_size / 12.5;
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        // 间距、内边距用 egui 默认值（界面一直是这个样子）
        let mut style = egui::Style { visuals: theme.default_visuals(), ..Default::default() };
        // 这套配色用在哪个样式上（浅色主题改浅色样式，深色主题改深色样式）
        let themed = palette.filter(|c| c.dark == (theme == egui::Theme::Dark));
        if let Some(c) = themed {
            warm_visuals(&mut style.visuals, c);
        }
        for font in style.text_styles.values_mut() {
            font.size *= scale;
        }
        // 小号字（标签、文件名、提示等）：egui 默认只有正文的 72%，偏小；按设置的百分比，随字号缩放
        let body = style.text_styles[&egui::TextStyle::Body].size;
        if let Some(small) = style.text_styles.get_mut(&egui::TextStyle::Small) {
            small.size = body * p.small_pct / 100.0;
        }
        // 光标不闪时，有输入框获得焦点也不会一直重画
        style.visuals.text_cursor.blink = p.cursor_blink;
        // 强调色：选了预设就用预设；选「默认蓝」时，配色主题用自己的强调色
        let preset = ACCENTS.get(p.accent as usize).and_then(|a| a.1).map(|(r, g, b)| Color32::from_rgb(r, g, b));
        if let Some(base) = preset.or_else(|| themed.and_then(|c| c.accent)) {
            let dark = theme == egui::Theme::Dark;
            let v = &mut style.visuals;
            v.selection.bg_fill = if dark { mix(base, Color32::BLACK, 0.45) } else { mix(base, Color32::WHITE, 0.55) };
            v.selection.stroke.color = if dark { mix(base, Color32::WHITE, 0.6) } else { mix(base, Color32::BLACK, 0.35) };
            v.hyperlink_color = v.selection.stroke.color;
        }
        ctx.set_style_of(theme, style);
    }
}

// ---------- 配色主题（护眼、樱米、薰衣草、深海、薄荷、羊皮纸）----------
// 都是浅色模式下的一套配色：纸色背景、深棕文字；线条（边框、分隔线、卡片轮廓）用明显的棕色。

/// 一套暖色主题的配色
struct Warm {
    /// 窗口、面板背景（主色）
    bg: Color32,
    /// 弹出窗口（设置、确认框）背景
    window: Color32,
    /// 输入框背景
    input: Color32,
    faint: Color32,
    /// 按钮：平时 / 悬停 / 按下
    button: [Color32; 3],
    /// 卡片底色：平时 / 悬停、选中
    card: [Color32; 2],
    text: Color32,
    /// 线条：分隔线、按钮和输入框边框
    line: Color32,
    /// 线条（悬停时，更深）
    line_strong: Color32,
    /// 卡片轮廓
    card_line: Color32,
    /// 主题自带的强调色：强调色选「默认蓝」时用它；选了别的强调色就以选的为准
    accent: Option<Color32>,
    /// 深色主题（深底浅字）：放在深色样式里，标签等也按深色配
    dark: bool,
}

/// 由三种颜色推出一整套配色：背景、文字、线条，再加上主题的强调色。
/// 卡片、按钮、输入框等都是背景往文字（或白 / 黑）方向调一点，层次和手调的两套护眼一致。
fn derive_theme(bg: Color32, text: Color32, line: Color32, accent: Color32, dark: bool) -> Warm {
    let (lift, sink) = if dark { (text, Color32::BLACK) } else { (Color32::WHITE, text) };
    Warm {
        bg,
        window: mix(bg, lift, if dark { 0.06 } else { 0.25 }),
        input: if dark { mix(bg, sink, 0.3) } else { mix(bg, lift, 0.5) },
        faint: mix(bg, text, 0.06),
        button: [mix(bg, text, 0.10), mix(bg, text, 0.16), mix(bg, text, 0.24)],
        card: [mix(bg, text, 0.06), mix(bg, text, 0.12)],
        text,
        line,
        line_strong: mix(line, text, 0.5),
        card_line: mix(bg, line, 0.5),
        accent: Some(accent),
        dark,
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// 主题编号 3 起对应的配色（顺序要和设置里的选项一致）
static WARM_THEMES: std::sync::LazyLock<Vec<Warm>> = std::sync::LazyLock::new(|| {
    vec![
    // 3 护眼：偏灰的卡其色，比较沉
    Warm {
        bg: rgb(208, 195, 165),
        window: rgb(214, 201, 172),
        input: rgb(222, 211, 186),
        faint: rgb(199, 185, 154),
        button: [rgb(195, 180, 147), rgb(185, 169, 134), rgb(172, 155, 118)],
        card: [rgb(198, 184, 152), rgb(187, 171, 137)],
        text: rgb(42, 30, 16),
        line: rgb(118, 95, 62),
        line_strong: rgb(75, 57, 32),
        card_line: rgb(152, 130, 94),
        accent: None,
        dark: false,
    },
    // 5 樱米：#F6E4C8 米色底，#E9527D 樱粉强调，#015697 深蓝做文字和线条
    derive_theme(
        rgb(0xF6, 0xE4, 0xC8),
        mix(rgb(0x01, 0x56, 0x97), Color32::BLACK, 0.5),
        mix(rgb(0x01, 0x56, 0x97), rgb(0xF6, 0xE4, 0xC8), 0.3),
        rgb(0xE9, 0x52, 0x7D),
        false,
    ),
    // 6 薰衣草：#DBD0E6 淡紫底，#161823 文字，#4F4F85 强调和线条
    derive_theme(
        rgb(0xDB, 0xD0, 0xE6),
        rgb(0x16, 0x18, 0x23),
        mix(rgb(0x4F, 0x4F, 0x85), rgb(0xDB, 0xD0, 0xE6), 0.2),
        rgb(0x4F, 0x4F, 0x85),
        false,
    ),
    // 7 深海（深色）：#0E4258 深青底，#FED3A8 杏色文字，#7E3427 赭红强调
    derive_theme(
        rgb(0x0E, 0x42, 0x58),
        rgb(0xFE, 0xD3, 0xA8),
        mix(rgb(0xFE, 0xD3, 0xA8), rgb(0x0E, 0x42, 0x58), 0.55),
        rgb(0x7E, 0x34, 0x27),
        true,
    ),
    // 8 薄荷：#E0F0E9 薄荷白底，#133E34 墨绿文字，#0D4D76 深蓝强调和线条
    derive_theme(
        rgb(0xE0, 0xF0, 0xE9),
        rgb(0x13, 0x3E, 0x34),
        mix(rgb(0x0D, 0x4D, 0x76), rgb(0xE0, 0xF0, 0xE9), 0.3),
        rgb(0x0D, 0x4D, 0x76),
        false,
    ),
    // 9 羊皮纸（待选色）：#E5D5B6 羊皮纸底，#B05B24 焦橙强调和线条，#161823 文字
    derive_theme(
        rgb(0xE5, 0xD5, 0xB6),
        rgb(0x16, 0x18, 0x23),
        mix(rgb(0xB0, 0x5B, 0x24), rgb(0xE5, 0xD5, 0xB6), 0.35),
        rgb(0xB0, 0x5B, 0x24),
        false,
    ),
    ]
});

/// 主题编号对应的配色（3 起），跟随系统 / 浅色 / 深色是 None
fn warm_of_theme(theme: u8) -> Option<&'static Warm> {
    // 编号不变（设置文件里存的是编号）：4 是已经移除的「护眼（浅）」，跳过
    const IDS: [u8; 6] = [3, 5, 6, 7, 8, 9];
    IDS.iter().position(|&id| id == theme).and_then(|i| WARM_THEMES.get(i))
}

fn warm_visuals(v: &mut egui::Visuals, c: &Warm) {
    v.panel_fill = c.bg;
    v.window_fill = c.window;
    v.extreme_bg_color = c.input;
    v.faint_bg_color = c.faint;
    v.code_bg_color = c.button[0];
    v.window_stroke = Stroke::new(1.0_f32, c.line);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = c.bg;
    // 灰色小字是文字颜色往这个颜色淡化出来的
    w.noninteractive.weak_bg_fill = c.bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, c.line);
    w.noninteractive.fg_stroke.color = c.text;
    for (s, fill) in [
        (&mut w.inactive, c.button[0]),
        (&mut w.hovered, c.button[1]),
        (&mut w.active, c.button[2]),
        (&mut w.open, c.button[1]),
    ] {
        s.bg_fill = fill;
        s.weak_bg_fill = fill;
        s.fg_stroke.color = c.text;
    }
    w.inactive.bg_stroke = Stroke::new(1.0_f32, c.line);
    w.hovered.bg_stroke = Stroke::new(1.0_f32, c.line_strong);
    w.active.bg_stroke = Stroke::new(1.0_f32, c.line_strong);
    w.open.bg_stroke = Stroke::new(1.0_f32, c.line);
}

/// 当前是哪套暖色主题（看面板底色，不用另外传参数）；不是暖色主题返回 None
fn warm(v: &egui::Visuals) -> Option<&'static Warm> {
    // 底色相同的主题（以前有过）再看弹出窗口的底色来区分
    WARM_THEMES.iter().find(|c| c.bg == v.panel_fill && c.window == v.window_fill)
}

/// 当前外观设置（每帧开头放进 egui 的临时数据，画卡片等的函数从这里读）
fn uip(ui: &Ui) -> UiPrefs {
    ui.ctx().data(|d| d.get_temp(Id::new("ui_prefs"))).unwrap_or_default()
}

// ---------- 中文字体 ----------

fn setup_fonts(ctx: &egui::Context, root: &Path) {
    let mut candidates: Vec<PathBuf> = vec![
        root.join(".sucard/font.ttf"),
        root.join(".sucard/font.ttc"),
        root.join(".sucard/font.otf"),
    ];
    for p in [
        // Windows
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyh.ttf",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simsun.ttc",
        // macOS
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
        // Linux
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
    ] {
        candidates.push(PathBuf::from(p));
    }

    for p in candidates {
        if let Ok(bytes) = std::fs::read(&p) {
            let mut fonts = egui::FontDefinitions::default();
            fonts
                .font_data
                .insert("cjk".to_owned(), egui::FontData::from_owned(bytes).into());
            for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                fonts.families.entry(fam).or_default().push("cjk".to_owned());
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
}

// ---------- 界面状态 ----------

#[derive(Clone, PartialEq)]
enum Sel {
    All,
    File(PathBuf),
    Tag(String),
    Untagged,
    Trash,
}

/// 等待确认的删除操作（确认框：回车 = 确认，Esc / 点框外 = 取消）
#[derive(Clone)]
enum Confirm {
    /// 删除一条素材
    Item(String),
    /// 删除文档（移到回收站）
    Doc(PathBuf),
    /// 彻底删除回收站里的文档（不可恢复）
    Purge(Vec<String>),
    /// 原文改了没保存就要离开；Some = 离开后要切换到的位置，None = 只是返回
    LeaveRaw(Option<Sel>),
}

/// 阅读原文的状态：只读，文件在外面（或右栏）被改了会自动重新读
struct RawRead {
    path: PathBuf,
    text: String,
    modified: Option<std::time::SystemTime>,
}

impl RawRead {
    fn open(store: &Store, path: PathBuf) -> Self {
        let modified = std::fs::metadata(store.root.join(&path)).and_then(|m| m.modified()).ok();
        RawRead { text: store.read_raw(&path), path, modified }
    }
    /// 文件修改时间变了就重新读（每帧只取一次修改时间，很便宜）
    fn refresh(&mut self, store: &Store) {
        let m = std::fs::metadata(store.root.join(&self.path)).and_then(|m| m.modified()).ok();
        if m != self.modified {
            *self = RawRead::open(store, self.path.clone());
        }
    }
}

/// 编辑原文的状态
struct RawEdit {
    path: PathBuf,
    buf: String,
    /// 打开（或上次保存）时的内容，用来判断有没有改动
    orig: String,
    /// (行数, 字数, 素材条数)，文字变了才重新统计
    stats: (usize, usize, usize),
}

impl RawEdit {
    fn new(path: PathBuf, text: String) -> Self {
        let mut r = RawEdit { path, buf: text.clone(), orig: text, stats: (0, 0, 0) };
        r.recount();
        r
    }
    fn dirty(&self) -> bool {
        self.buf != self.orig
    }
    fn recount(&mut self) {
        let lines = self.buf.lines().count();
        let chars = self.buf.chars().filter(|c| !c.is_whitespace()).count();
        let items = self.buf.lines().filter(|l| parse_task(l).is_some()).count();
        self.stats = (lines, chars, items);
    }
}

/// 拖动时携带的条目 id
#[derive(Clone)]
struct DragItem(String);

enum Act {
    SetDone(String, bool),
    /// 真正删除（确认框里点了「删除」）
    Delete(String),
    /// 请求删除：先弹确认框
    AskDelete(String),
    /// 删除文档：移到回收站（确认后）
    DeleteDoc(PathBuf),
    /// 彻底删除回收站里的文档（确认后）
    Purge(Vec<String>),
    /// 从回收站还原
    Restore(String),
    MoveTo(String, PathBuf),
    AddTag(String, String, bool),
    SetPrimary(String, Option<String>),
    Reorder(String, String, bool),
    StartEdit(String, u8),
    CommitEdit,
    Select(String, u8),
    ToggleTag(String, String),
}

struct App {
    store: Store,
    sel: Sel,
    quick: String,
    quick_focus: bool,
    search: String,
    /// (条目 id, 编辑中的文字, 所在位置：0 中间 / 1 右侧)
    editing: Option<(String, String, u8)>,
    edit_focus: bool,
    /// 选中的条目和所在位置（0 中间 / 1 右侧）；两栏里同一条都会高亮
    selected: Option<(String, u8)>,
    raw: Option<RawEdit>,
    reading: Option<RawRead>,
    /// 正在改名的文件和输入框里的新名字；只在选中的仍是这个文件时显示
    rename: Option<(PathBuf, String)>,
    rename_focus: bool,
    new_doc: String,
    settings_open: bool,
    /// 等待确认的删除操作
    confirm: Option<Confirm>,
    /// 回收站里勾选的文档（批量彻底删除用）
    trash_checked: std::collections::BTreeSet<String>,
    /// 外观设置已应用到 egui（第一帧时应用）
    ui_applied: bool,
    /// 外观设置改了还没写盘（拖动条拖动中不写，松手后写一次）
    ui_dirty: bool,
    /// 虚拟滚动量出来的每行高度（含行间距）：[0] 中间栏，[1] 右栏
    row_h: [HashMap<String, f32>; 2],
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, root: PathBuf) -> Self {
        setup_fonts(&cc.egui_ctx, &root);
        // 第一次使用这个数据目录（还没有 .sucard）：生成一份使用说明
        let first_run = !root.join(".sucard").exists();
        // 已经有使用说明的（比如以前用过）就不再生成第二份
        let has_guide = root.join(store::GUIDE).exists();
        let mut store = Store::open(root);
        if first_run && !has_guide {
            store.install_guide();
        }
        App::with_store(store)
    }

    fn with_store(store: Store) -> Self {
        App {
            store,
            sel: Sel::All,
            quick: String::new(),
            quick_focus: true,
            search: String::new(),
            editing: None,
            edit_focus: false,
            selected: None,
            raw: None,
            reading: None,
            rename: None,
            rename_focus: false,
            settings_open: false,
            confirm: None,
            trash_checked: Default::default(),
            ui_applied: false,
            ui_dirty: false,
            row_h: Default::default(),
            new_doc: String::new(),
        }
    }

    /// 外部文件拖进窗口：拖动中显示提示，松开后复制进素材库（数据目录）。
    fn handle_file_drop(&mut self, ctx: &egui::Context) {
        let hovered = ctx.input(|i| i.raw.hovered_files.len());
        if hovered > 0 {
            let screen = ctx.screen_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("drop_hint")));
            let v = ctx.style().visuals.clone();
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(if v.dark_mode { 150 } else { 90 }));
            let text = format!("松开以导入 {hovered} 个文件到素材库\n只导入 .md / .txt，原文件不会被改动");
            let galley = painter.layout(
                text,
                egui::FontId::proportional(18.0),
                Color32::WHITE,
                screen.width() - 80.0,
            );
            let pos = screen.center() - galley.size() / 2.0;
            let bg = egui::Rect::from_min_size(pos, galley.size()).expand(18.0);
            painter.rect_filled(bg, 10.0, v.selection.stroke.color.gamma_multiply(0.9));
            painter.galley(pos, galley, Color32::WHITE);
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if dropped.is_empty() {
            return;
        }
        let mut last = None;
        let mut failed = Vec::new();
        for src in &dropped {
            match self.store.import_file(src) {
                Ok(rel) => last = Some(rel),
                Err(e) => failed.push(e),
            }
        }
        if last.is_some() {
            self.store.reload();
        }
        if !failed.is_empty() {
            self.store.error = Some(format!("以下文件没有导入：{}", failed.join("、")));
        }
        if let Some(p) = last {
            self.goto(Sel::File(p));
        }
        ctx.request_repaint();
    }

    /// 切换左栏选择。正在编辑原文且没保存时，先弹框问（保存 / 不保存 / 继续编辑）
    fn goto(&mut self, sel: Sel) {
        if self.raw.as_ref().is_some_and(|r| r.dirty()) {
            self.confirm = Some(Confirm::LeaveRaw(Some(sel)));
            return;
        }
        self.sel = sel;
        self.raw = None;
        self.reading = None;
    }

    /// 保存原文，留在编辑区。保存后重新读一遍：新条目会被补上 ^id
    fn save_raw(&mut self) {
        let Some(r) = &self.raw else { return };
        let (path, content) = (r.path.clone(), r.buf.clone());
        self.store.write_raw(&path, &content);
        self.editing = None;
        if self.store.error.is_none() {
            let text = self.store.read_raw(&path);
            if let Some(r) = &mut self.raw {
                r.buf = text.clone();
                r.orig = text;
                r.recount();
            }
        }
    }

    /// 左栏的一个文档块：点击选中，拖卡片进来 = 移入，右键删除（固定文档不能删）
    fn file_block(&mut self, ui: &mut Ui, path: &Path, indent: f32, label: String, acts: &mut Vec<Act>) {
        let n = self.store.items.values().filter(|it| !it.done && it.file == path).count();
        let selected = matches!(&self.sel, Sel::File(p) if p == path);
        let in_todo = self.store.in_todo(path);
        let text = if in_todo { RichText::new(label) } else { RichText::new(label).color(ui.visuals().weak_text_color()) };
        let resp = nav_block(ui, ("nav_file", path), selected, indent, text, n);
        let resp = if in_todo { resp } else { resp.on_hover_text("不放入TODO：条目不在右栏显示（右键可改）") };
        if resp.clicked() {
            self.goto(Sel::File(path.to_path_buf()));
        }
        if resp.dnd_hover_payload::<DragItem>().is_some() {
            drop_highlight(ui, resp.rect);
        }
        if let Some(p) = resp.dnd_release_payload::<DragItem>() {
            acts.push(Act::MoveTo(p.0.clone(), path.to_path_buf()));
        }
        let fixed = is_fixed_doc(path);
        let mut in_todo = self.store.in_todo(path);
        let mut todo_changed = false;
        resp.context_menu(|ui| {
            if clear_checkbox(ui, &mut !in_todo.clone(), "不放入TODO")
                .on_hover_text("勾选后，这个文档的条目不在右栏 TODO 里显示")
                .clicked()
            {
                in_todo = !in_todo;
                todo_changed = true;
                ui.close_menu();
            }
            ui.separator();
            let r = ui.add_enabled(!fixed, egui::Button::new("删除文档…"));
            if fixed {
                r.on_disabled_hover_text("固定文档，不能删除");
            } else if r.on_hover_text("移到回收站，可以还原").clicked() {
                self.confirm = Some(Confirm::Doc(path.to_path_buf()));
                ui.close_menu();
            }
        });
        if todo_changed {
            self.store.set_in_todo(path, in_todo);
        }
    }

    /// 快速添加的目标文件：选中的文件，否则收件箱
    fn target_file(&self) -> PathBuf {
        match &self.sel {
            Sel::File(p) => p.clone(),
            _ => PathBuf::from(INBOX),
        }
    }

    fn visible(&self, it: &Item) -> bool {
        if self.store.prefs.hide_done && it.done {
            return false;
        }
        let q = self.search.trim();
        q.is_empty() || it.text.to_lowercase().contains(&q.to_lowercase())
    }

    /// 快速添加左边的「# 标签」按钮：和卡片右键「编辑标签…」同一个面板，
    /// 改的是输入框里的文字（回车保存时才写入）。可以连点多个，点外面关闭。
    fn tag_picker(&mut self, ui: &mut Ui) {
        let chosen = store::parse_tags(&self.quick);
        let title = if chosen.is_empty() {
            "# 标签 ▼".to_string()
        } else {
            format!("# 标签 {} ▼", chosen.len())
        };
        ui.menu_button(title, |ui| {
            match tag_panel(ui, &chosen, &self.store, Id::new("new_tag_quick")) {
                Some(TagOp::Toggle(t)) => {
                    self.quick = store::toggle_tag(&self.quick, &t);
                    // 光标放回输入框末尾，方便接着打字
                    self.quick_focus = true;
                }
                Some(TagOp::Add(t)) => {
                    // 新建的标签：没有才加。焦点留在面板的输入框里，可以接着新建
                    if !chosen.contains(&t) {
                        self.quick = store::toggle_tag(&self.quick, &t);
                    }
                }
                Some(TagOp::Clear) => {
                    self.quick = store::strip_tags(&self.quick);
                    self.quick_focus = true;
                }
                None => {}
            }
        });
    }

    /// 选中条目后的键盘操作：空格 完成/未完成，Delete 删除，回车 编辑，Esc 取消选中。
    /// 有输入框在打字时不处理。按键先被吃掉，免得勾选框等控件再响应一次。
    fn handle_keys(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        // 删除确认框开着：回车、Esc 归确认框
        if self.confirm.is_some() {
            return;
        }
        let Some((id, place)) = self.selected.clone() else { return };
        let Some(done) = self.store.items.get(&id).map(|it| it.done) else {
            self.selected = None;
            return;
        };
        if ctx.wants_keyboard_input() {
            return;
        }
        let key = |k: Key| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
        if key(Key::Space) {
            acts.push(Act::SetDone(id, !done));
        } else if key(Key::Delete) {
            acts.push(Act::AskDelete(id));
        } else if key(Key::Enter) {
            acts.push(Act::StartEdit(id, place));
        } else if key(Key::Escape) {
            self.selected = None;
        }
    }

    fn sel_match(&self, it: &Item) -> bool {
        match &self.sel {
            Sel::All => true,
            Sel::File(p) => &it.file == p,
            Sel::Tag(t) => it.has_tag(t),
            Sel::Untagged => it.tags().is_empty(),
            Sel::Trash => false,
        }
    }

    fn apply(&mut self, acts: Vec<Act>) {
        for a in acts {
            match a {
                Act::SetDone(id, d) => self.store.set_done(&id, d),
                Act::Select(id, place) => self.selected = Some((id, place)),
                Act::AskDelete(id) => self.confirm = Some(Confirm::Item(id)),
                Act::DeleteDoc(p) => {
                    if self.store.delete_doc(&p) {
                        if self.sel == Sel::File(p.clone()) {
                            self.sel = Sel::All;
                        }
                        if self.raw.as_ref().is_some_and(|r| r.path == p) {
                            self.raw = None;
                        }
                        if self.reading.as_ref().is_some_and(|r| r.path == p) {
                            self.reading = None;
                        }
                        self.rename = None;
                        // 正在编辑 / 选中的条目可能已经跟着文档没了
                        if self.editing.as_ref().is_some_and(|(e, _, _)| !self.store.items.contains_key(e)) {
                            self.editing = None;
                        }
                        if self.selected.as_ref().is_some_and(|(s, _)| !self.store.items.contains_key(s)) {
                            self.selected = None;
                        }
                    }
                }
                Act::Purge(files) => {
                    self.store.purge_trash(&files);
                    self.trash_checked.retain(|f| !files.contains(f));
                }
                Act::Restore(f) => {
                    self.trash_checked.remove(&f);
                    if self.store.restore_trash(&f).is_some() {
                        // 还原后重新读取了全部数据
                        self.editing = None;
                        self.raw = None;
                        self.reading = None;
                    }
                }
                Act::ToggleTag(id, t) => self.store.toggle_item_tag(&id, &t),
                Act::Delete(id) => {
                    if self.editing.as_ref().is_some_and(|(e, _, _)| *e == id) {
                        self.editing = None;
                    }
                    if self.selected.as_ref().is_some_and(|(s, _)| *s == id) {
                        self.selected = None;
                    }
                    self.store.delete(&id);
                }
                Act::MoveTo(id, p) => self.store.move_to(&id, &p),
                Act::AddTag(id, t, replace) => self.store.add_tag(&id, &t, replace),
                Act::SetPrimary(id, t) => self.store.set_primary_tag(&id, t.as_deref()),
                Act::Reorder(id, target, after) => self.store.reorder(&id, &target, after),
                Act::StartEdit(id, place) => {
                    if let Some(it) = self.store.items.get(&id) {
                        self.editing = Some((id, it.text.clone(), place));
                        self.edit_focus = true;
                    }
                }
                Act::CommitEdit => {
                    if let Some((id, text, _)) = self.editing.take() {
                        if text.trim().is_empty() {
                            // 内容删光了：也要确认；取消的话这条保留原来的内容
                            self.confirm = Some(Confirm::Item(id));
                        } else {
                            self.store.set_text(&id, &text);
                        }
                    }
                }
            }
        }
    }
}

// ---------- 小部件 ----------

fn tag_colors(tag: &str, dark: bool) -> (Color32, Color32) {
    // (背景, 文字)
    const LIGHT: [(u8, u8, u8, u8, u8, u8); 6] = [
        (238, 237, 254, 60, 52, 137),  // 紫
        (225, 245, 238, 8, 80, 65),    // 青
        (250, 236, 231, 113, 43, 19),  // 橙
        (230, 241, 251, 12, 68, 124),  // 蓝
        (251, 234, 240, 114, 36, 62),  // 粉
        (234, 243, 222, 39, 80, 10),   // 绿
    ];
    const DARK: [(u8, u8, u8, u8, u8, u8); 6] = [
        (60, 52, 137, 206, 203, 246),
        (8, 80, 65, 159, 225, 203),
        (113, 43, 19, 245, 196, 179),
        (12, 68, 124, 181, 212, 244),
        (114, 36, 62, 244, 192, 209),
        (39, 80, 10, 192, 221, 151),
    ];
    let root = tag.split('/').next().unwrap_or(tag);
    let h = root.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32) as usize % 6;
    let c = if dark { DARK[h] } else { LIGHT[h] };
    (Color32::from_rgb(c.0, c.1, c.2), Color32::from_rgb(c.3, c.4, c.5))
}

fn tag_chip(ui: &mut Ui, tag: &str) {
    let (bg, fg) = tag_colors(tag, ui.visuals().dark_mode);
    ui.label(RichText::new(format!(" #{tag} ")).small().color(fg).background_color(bg));
}

fn file_label(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// 条目块的底色：悬停或选中时加深，已完成的变淡
fn block_fill(v: &egui::Visuals, strong: bool, done: bool) -> Color32 {
    let (base, deep) = if let Some(c) = warm(v) {
        (c.card[0], c.card[1])
    } else if v.dark_mode {
        (Color32::from_gray(38), Color32::from_gray(50))
    } else {
        (Color32::from_gray(238), Color32::from_gray(226))
    };
    let c = if strong { deep } else { base };
    if done {
        c.gamma_multiply(0.55)
    } else {
        c
    }
}

/// 一个条目块：底色卡片，里面是拖动柄、勾选框、正文和标签。
/// 整块（含空白处）可以单击选中、双击编辑、右键菜单、拖动；
/// 整块的感应区注册在子控件下面，所以勾选框和编辑框照常接收点击。
/// 返回整块区域，供放置目标使用。
#[allow(clippy::too_many_arguments)]
/// 把 job 里落在 ranges 中的部分切出来，用 f 改格式（其余部分不动）
fn restyle(
    job: &mut egui::text::LayoutJob,
    ranges: &[std::ops::Range<usize>],
    f: impl Fn(&mut egui::text::TextFormat),
) {
    if ranges.is_empty() {
        return;
    }
    let mut sections = Vec::with_capacity(job.sections.len() + ranges.len() * 2);
    for sec in std::mem::take(&mut job.sections) {
        let mut start = sec.byte_range.start;
        let end = sec.byte_range.end;
        for r in ranges {
            if r.end <= start || r.start >= end {
                continue;
            }
            let (a, b) = (r.start.max(start), r.end.min(end));
            if a > start {
                sections.push(egui::text::LayoutSection { byte_range: start..a, ..sec.clone() });
            }
            let mut format = sec.format.clone();
            f(&mut format);
            sections.push(egui::text::LayoutSection { leading_space: 0.0, byte_range: a..b, format });
            start = b;
        }
        if start < end {
            sections.push(egui::text::LayoutSection { byte_range: start..end, ..sec });
        }
    }
    job.sections = sections;
}

/// 用浏览器打开网址；www. 开头的补上 https://
fn open_link(ctx: &egui::Context, url: &str) {
    let url = if url.starts_with("www.") { format!("https://{url}") } else { url.to_string() };
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

/// 文字里所有网址（去重）
fn urls_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in find_urls(text) {
        let u = text[r].to_string();
        if !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

/// 菜单里显示的网址：去掉 https://，太长截断
fn short_url(u: &str) -> String {
    let s = u.trim_start_matches("https://").trim_start_matches("http://");
    if s.chars().count() > 36 {
        format!("{}…", s.chars().take(36).collect::<String>())
    } else {
        s.to_string()
    }
}

/// 原文着色用到的样式（变了才重新着色）
#[derive(Hash)]
struct HiStyle {
    size_bits: u32,
    dark: bool,
    text: Color32,
    weak: Color32,
    accent: Color32,
    link: Color32,
    mark: Color32,
}

/// ==标记== 的底色（类似荧光笔）
fn mark_bg(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(105, 90, 25)
    } else {
        Color32::from_rgb(255, 236, 140)
    }
}

impl HiStyle {
    fn of(ui: &Ui) -> Self {
        let v = ui.visuals();
        HiStyle {
            size_bits: ui.text_style_height(&egui::TextStyle::Body).to_bits(),
            dark: v.dark_mode,
            text: v.text_color(),
            weak: v.weak_text_color(),
            accent: v.selection.stroke.color,
            link: v.hyperlink_color,
            mark: mark_bg(v.dark_mode),
        }
    }
}

#[derive(Default)]
struct Highlighter;

impl egui::cache::ComputerMut<(&HiStyle, &str), egui::text::LayoutJob> for Highlighter {
    fn compute(&mut self, (style, text): (&HiStyle, &str)) -> egui::text::LayoutJob {
        highlight_markdown(style, text)
    }
}

/// 着色结果按（样式, 全文）缓存：文字没变的帧直接复用
type HiCache = egui::cache::FrameCache<egui::text::LayoutJob, Highlighter>;

/// 原文轻量着色：
/// `- [ ]` / `- [x]` 强调色，已完成的条目（连同续行）变灰加删除线；
/// `#标签` 用标签色；任务行末尾的 `^id` 变淡。
/// 底色比换字色显眼：`[ ]` 笔画细，只换颜色看不出来。
fn highlight_markdown(st: &HiStyle, text: &str) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let size = f32::from_bits(st.size_bits);
    let fmt = |color: Color32, size: f32, strike: bool| TextFormat {
        font_id: egui::FontId::proportional(size),
        color,
        strikethrough: if strike { Stroke::new(1.0_f32, color) } else { Stroke::NONE },
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    // 当前所在条目：Some((任务行缩进, 是否已完成))
    let mut item: Option<(usize, bool)> = None;

    for raw_line in text.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\n', '\r']);
        let eol = &raw_line[line.len()..];
        let indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>();

        if let Some((prefix, done, _, _)) = parse_task(line) {
            item = Some((indent, done));
            let head = prefix.len() + 3; // 「- 」+「[ ]」
            let (box_part, rest) = line.split_at(head.min(line.len()));
            let mut f = fmt(if done { st.weak } else { st.accent }, size, false);
            f.background = if done { st.weak.gamma_multiply(0.25) } else { st.accent.gamma_multiply(0.3) };
            job.append(box_part, 0.0, f);
            highlight_words(&mut job, rest, st, size, done, true, &fmt);
        } else {
            let cont = !line.trim().is_empty() && item.is_some_and(|(ti, _)| indent > ti);
            if !cont {
                item = None;
            }
            let done = cont && item.is_some_and(|(_, d)| d);
            highlight_words(&mut job, line, st, size, done, false, &fmt);
        }
        if !eol.is_empty() {
            job.append(eol, 0.0, fmt(st.text, size, false));
        }
    }
    // %%备注%%（可跨行）整段改成淡色，盖掉上面的着色
    let comments: Vec<_> = split_comments(text).into_iter().filter(|(_, c)| *c).map(|(r, _)| r).collect();
    let comment_fmt = fmt(st.weak, size, false);
    restyle(&mut job, &comments, |f| *f = comment_fmt.clone());
    restyle(&mut job, &find_marks(text), |f| f.background = st.mark);
    // 网址：下划线；备注里、已完成的保持灰色，其余用链接色
    restyle(&mut job, &find_urls(text), |f| {
        if f.color != st.weak {
            f.color = st.link;
        }
        f.underline = Stroke::new(1.0_f32, f.color);
    });
    job
}

/// 阅读原文的排版：
/// - `# 标题` 去掉 #，放大；`- [ ]` / `- [x]` 换成 □ / ■，已完成的整条变灰加删除线；
/// - 行末的 ^id 不显示；#标签 用标签色；%%备注%% 变灰；==标记== 黄色底色；网址链接色加下划线。
fn render_reading(st: &HiStyle, text: &str) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let size = f32::from_bits(st.size_bits);
    let fmt = |color: Color32, size: f32, strike: bool| TextFormat {
        font_id: egui::FontId::proportional(size),
        color,
        strikethrough: if strike { Stroke::new(1.0_f32, color) } else { Stroke::NONE },
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    let mut item: Option<(usize, bool)> = None;
    for raw_line in text.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\n', '\r']);
        let indent_str = &line[..line.len() - line.trim_start().len()];
        let indent = indent_str.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum::<usize>();
        if let Some((prefix, done, _, _)) = parse_task(line) {
            item = Some((indent, done));
            let rest = &line[(prefix.len() + 3).min(line.len())..];
            // 去掉行末的 ^id
            let rest = match rest.trim_end().rsplit_once(' ') {
                Some((head, last)) if is_block_id(last) => head,
                _ if is_block_id(rest.trim()) => "",
                _ => rest,
            };
            job.append(indent_str, 0.0, fmt(st.text, size, false));
            let mark = if done { "■" } else { "□" };
            job.append(mark, 0.0, fmt(if done { st.weak } else { st.accent }, size, false));
            highlight_words(&mut job, rest, st, size, done, false, &fmt);
        } else if indent == 0 && line.starts_with('#') && {
            let hashes = line.len() - line.trim_start_matches('#').len();
            hashes <= 6 && line[hashes..].starts_with(' ')
        } {
            item = None;
            let hashes = line.len() - line.trim_start_matches('#').len();
            let scale = if hashes == 1 { 1.35 } else { 1.18 };
            job.append(line[hashes..].trim(), 0.0, fmt(st.text, size * scale, false));
        } else {
            let cont = !line.trim().is_empty() && item.is_some_and(|(ti, _)| indent > ti);
            if !cont {
                item = None;
            }
            let done = cont && item.is_some_and(|(_, d)| d);
            highlight_words(&mut job, line, st, size, done, false, &fmt);
        }
        if raw_line.len() > line.len() {
            job.append("\n", 0.0, fmt(st.text, size, false));
        }
    }
    let t = job.text.clone();
    let comments: Vec<_> = split_comments(&t).into_iter().filter(|(_, c)| *c).map(|(r, _)| r).collect();
    let comment_fmt = fmt(st.weak, size, false);
    restyle(&mut job, &comments, |f| *f = comment_fmt.clone());
    restyle(&mut job, &find_marks(&t), |f| f.background = st.mark);
    restyle(&mut job, &find_urls(&t), |f| {
        if f.color != st.weak {
            f.color = st.link;
        }
        f.underline = Stroke::new(1.0_f32, f.color);
    });
    // 段落之间留点行距，读起来不挤
    job.sections.iter_mut().for_each(|s| s.format.line_height = Some(size * 1.6));
    job
}

#[derive(Default)]
struct ReadRenderer;

impl egui::cache::ComputerMut<(&HiStyle, &str), egui::text::LayoutJob> for ReadRenderer {
    fn compute(&mut self, (style, text): (&HiStyle, &str)) -> egui::text::LayoutJob {
        render_reading(style, text)
    }
}

type ReadCache = egui::cache::FrameCache<egui::text::LayoutJob, ReadRenderer>;

/// 一段文字按空白切开逐词着色（空白原样保留）
fn highlight_words(
    job: &mut egui::text::LayoutJob,
    s: &str,
    st: &HiStyle,
    size: f32,
    done: bool,
    task_line: bool,
    fmt: &dyn Fn(Color32, f32, bool) -> egui::text::TextFormat,
) {
    // 任务行最后一个词如果是 ^id，单独变淡
    let last_word_start = s.trim_end().rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
    let mut i = 0;
    while i < s.len() {
        let ws = s[i..].find(|c: char| !c.is_whitespace()).map(|n| i + n).unwrap_or(s.len());
        if ws > i {
            job.append(&s[i..ws], 0.0, fmt(st.text, size, false));
        }
        if ws >= s.len() {
            break;
        }
        let end = s[ws..].find(char::is_whitespace).map(|n| ws + n).unwrap_or(s.len());
        let word = &s[ws..end];
        let color = if task_line && ws == last_word_start && is_block_id(word) {
            st.weak.gamma_multiply(0.7)
        } else if done {
            st.weak
        } else if let Some(t) = tag_of_word(word) {
            tag_colors(t, st.dark).1
        } else {
            st.text
        };
        let strike = done && !is_block_id(word);
        job.append(word, 0.0, fmt(color, size, strike));
        i = end;
    }
}

/// 虚拟滚动：只画视口附近的行，其余的行只按高度留出空白。
/// 画过的行会量出真实高度记在 heights 里（含行间距），没画过的按平均高度估算，
/// 所以每行高度可以不一样。keep(i) 为真的行看不见也照画（正在编辑、正在拖动的）。
/// 必须在 ScrollArea::show_viewport 里调用，viewport 是它给的可见区域。
fn virtual_list(
    ui: &mut Ui,
    viewport: Rect,
    heights: &mut HashMap<String, f32>,
    keys: &[String],
    keep: impl Fn(usize) -> bool,
    mut draw: impl FnMut(&mut Ui, usize),
) {
    let est = if heights.is_empty() {
        ui.spacing().interact_size.y * 2.0 + 12.0
    } else {
        heights.values().sum::<f32>() / heights.len() as f32
    };
    // 视口上下多画一点，滚动时边缘不会空一下
    let margin = 200.0;
    let (lo, hi) = (viewport.min.y - margin, viewport.max.y + margin);
    let origin = ui.max_rect().top();
    let mut y = ui.cursor().top() - origin;
    let mut skip = 0.0;
    for (i, k) in keys.iter().enumerate() {
        let h = heights.get(k).copied().unwrap_or(est);
        if (y + h >= lo && y <= hi) || keep(i) {
            if skip > 0.0 {
                ui.add_space(skip);
                skip = 0.0;
            }
            let top = ui.cursor().top();
            draw(ui, i);
            let real = ui.cursor().top() - top;
            match heights.get_mut(k) {
                Some(v) => *v = real,
                None => {
                    heights.insert(k.clone(), real);
                }
            }
            y = ui.cursor().top() - origin;
        } else {
            skip += h;
            y += h;
        }
    }
    if skip > 0.0 {
        ui.add_space(skip);
    }
}

fn item_row(
    ui: &mut Ui,
    it: &Item,
    place: u8,
    show_file: bool,
    selected: bool,
    store: &Store,
    editing: &mut Option<(String, String, u8)>,
    edit_focus: &mut bool,
    acts: &mut Vec<Act>,
    draggable: bool,
) -> Rect {
    let sense = if draggable { Sense::click_and_drag() } else { Sense::click() };
    let block = ui.scope_builder(egui::UiBuilder::new().sense(sense), |ui| {
        // 文字不可选，否则在文字上按下会被当成选字，而不是拖动整块
        ui.style_mut().interaction.selectable_labels = false;
        let v = ui.visuals();
        let hovered = ui.response().contains_pointer();
        // 描边宽度固定为 1，只换颜色，选中时不会让布局跳动
        let stroke_color = if selected {
            v.selection.stroke.color
        } else if let Some(c) = warm(v) {
            c.card_line
        } else {
            Color32::TRANSPARENT
        };
        egui::Frame::new()
            .fill(block_fill(v, hovered || selected, it.done))
            .stroke(Stroke::new(1.0_f32, stroke_color))
            .corner_radius(uip(ui).radius)
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                item_row_content(ui, it, place, show_file, editing, edit_focus, acts)
            })
            .inner
    });

    let resp = block.response;
    let link_click = resp.clicked() && ui.input(|i| i.modifiers.command);
    if let (true, Some(u)) = (link_click, &block.inner) {
        open_link(ui.ctx(), u);
    } else if resp.clicked() {
        // 让顶部输入框等交出焦点，键盘快捷键才会作用到选中的条目
        ui.memory_mut(|m| {
            if let Some(f) = m.focused() {
                m.surrender_focus(f);
            }
        });
        acts.push(Act::Select(it.id.clone(), place));
    }
    if resp.double_clicked() {
        acts.push(Act::StartEdit(it.id.clone(), place));
    }
    if draggable {
        resp.dnd_set_drag_payload(DragItem(it.id.clone()));
    }
    // 「编辑标签」不用子菜单：卡片在窗口右边时，子菜单会被挤到左边、盖住主菜单，
    // 点标签可能点到下面的「删除」。改成同一个菜单里切换到标签面板。
    // 按卡片记（同一条在中间栏和右栏各有一张卡片，不能按条目 id 记）
    let tag_mode_key = resp.id.with("tag_mode");
    resp.context_menu(|ui| {
        if ui.data(|d| d.get_temp::<bool>(tag_mode_key)).unwrap_or(false) {
            if ui.button("← 返回").clicked() {
                ui.data_mut(|d| d.remove::<bool>(tag_mode_key));
            }
            ui.separator();
            tag_menu(ui, it, store, acts);
            return;
        }
        if ui.button("编辑").clicked() {
            acts.push(Act::StartEdit(it.id.clone(), place));
            ui.close_menu();
        }
        // 复制全部正文（多行完整复制），去掉 #标签
        if ui.button("复制全部").on_hover_text("复制全部正文，不含 #标签 和 %%备注%%").clicked() {
            ui.ctx().copy_text(strip_comments(&it.body()));
            ui.close_menu();
        }
        let part = marked_text(&it.body());
        let r = ui
            .add_enabled(!part.is_empty(), egui::Button::new("部分复制"))
            .on_hover_text("只复制 ==标记== 的内容")
            .on_disabled_hover_text("没有 ==标记== 的内容。编辑原文里选中文字点「复制选中」，或按 Ctrl+E");
        if r.clicked() {
            ui.ctx().copy_text(part);
            ui.close_menu();
        }
        // 网址（含备注里的出处）：只有一个时「打开链接」「复制链接」，多个时逐个列出
        let urls = urls_in(&it.text);
        if urls.len() == 1 {
            if ui.button("打开链接").on_hover_text(&urls[0]).clicked() {
                open_link(ui.ctx(), &urls[0]);
                ui.close_menu();
            }
            if ui.button("复制链接").on_hover_text(&urls[0]).clicked() {
                ui.ctx().copy_text(urls[0].clone());
                ui.close_menu();
            }
        } else if urls.len() > 1 {
            for u in urls.iter().take(8) {
                if ui.button(format!("打开 {}", short_url(u))).on_hover_text(u).clicked() {
                    open_link(ui.ctx(), u);
                    ui.close_menu();
                }
            }
            if ui.button(format!("复制全部链接（{}）", urls.len())).on_hover_text("每行一个").clicked() {
                ui.ctx().copy_text(urls.join("\n"));
                ui.close_menu();
            }
        }
        if ui.button("编辑标签…").clicked() {
            ui.data_mut(|d| d.insert_temp(tag_mode_key, true));
        }
        let label = if it.done { "标记为未完成" } else { "标记为已用 / 完成" };
        if ui.button(label).clicked() {
            acts.push(Act::SetDone(it.id.clone(), !it.done));
            ui.close_menu();
        }
        ui.separator();
        if ui.button("删除").clicked() {
            acts.push(Act::AskDelete(it.id.clone()));
            ui.close_menu();
        }
    });
    // 菜单关了，下次打开从主菜单开始
    if !resp.context_menu_opened() {
        ui.data_mut(|d| d.remove::<bool>(tag_mode_key));
    }
    resp.rect
}

/// 标签面板里的操作
enum TagOp {
    /// 点了某个标签：有就去掉，没有就加上
    Toggle(String),
    /// 在输入框里新建了一个标签
    Add(String),
    /// 清除全部标签
    Clear,
}

/// 标签面板：顶部新建标签，中间是全部标签（已有的高亮、父标签提示「已有子标签」），
/// 底部清除全部。卡片右键「编辑标签…」和快速添加的「# 标签」共用，返回点了什么，由调用方去执行。
/// have：当前已有的标签；key：新建输入框的内容存放位置（菜单开着时保留）。
/// 标签列表只在面板打开时才统计。
fn tag_panel(ui: &mut Ui, have: &[String], store: &Store, key: Id) -> Option<TagOp> {
    let mut op = None;

    // 新建标签
    let mut buf: String = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let r = ui
        .scope(|ui| {
            input_visuals(ui);
            ui.add(
                TextEdit::singleline(&mut buf)
                    .margin(input_margin())
                    .hint_text("新标签，回车添加")
                    .desired_width(180.0),
            )
        })
        .inner;
    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        // 标签不能有空格和开头的 #：「#AI 工具」→「AI工具」
        let t: String = buf.trim().trim_start_matches('#').split_whitespace().collect();
        let t = t.trim_matches('/').to_string();
        if !t.is_empty() {
            op = Some(TagOp::Add(t));
        }
        buf.clear();
        r.request_focus();
    }
    ui.data_mut(|d| d.insert_temp(key, buf));

    let tags = store.all_tags();
    if !tags.is_empty() {
        ui.separator();
    }
    ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
        for t in tags {
            let depth = t.matches('/').count();
            let short = t.rsplit('/').next().unwrap_or(&t);
            let on = have.contains(&t);
            // 本身没有、但有它的子标签：#资料/AI 之于 资料
            let child = format!("{t}/");
            let kids: Vec<&String> = have.iter().filter(|h| h.starts_with(&child)).collect();
            let (_, fg) = tag_colors(&t, ui.visuals().dark_mode);
            let weak = ui.visuals().weak_text_color();
            let mut text = egui::text::LayoutJob::default();
            let font = egui::TextStyle::Body.resolve(ui.style());
            text.append(&format!("# {short}"), 0.0, egui::TextFormat::simple(font.clone(), fg));
            if !on && !kids.is_empty() {
                text.append(" · 已有子标签", 0.0, egui::TextFormat::simple(font, weak));
            }
            let hover = if on {
                format!("#{t}
点击去掉")
            } else if !kids.is_empty() {
                let names: Vec<String> = kids.iter().map(|k| format!("#{k}")).collect();
                format!("已有 {}
点击会再加上 #{t} 本身", names.join(" "))
            } else {
                format!("#{t}
点击加上")
            };
            let clicked = ui
                .horizontal(|ui| {
                    ui.add_space(depth as f32 * 14.0);
                    ui.selectable_label(on, text).on_hover_text(hover).clicked()
                })
                .inner;
            if clicked {
                op = Some(TagOp::Toggle(t));
            }
        }
    });

    if !have.is_empty() {
        ui.separator();
        if ui.button("清除全部标签").clicked() {
            op = Some(TagOp::Clear);
            ui.close_menu();
        }
    }
    op
}

/// 卡片右键「编辑标签…」：每点一下立即保存；菜单不关，可以连续改（换标签 = 去掉旧的 + 点上新的）。
fn tag_menu(ui: &mut Ui, it: &Item, store: &Store, acts: &mut Vec<Act>) {
    let key = Id::new(("new_tag", it.id.as_str()));
    match tag_panel(ui, &it.tags(), store, key) {
        Some(TagOp::Toggle(t)) => acts.push(Act::ToggleTag(it.id.clone(), t)),
        Some(TagOp::Add(t)) => acts.push(Act::AddTag(it.id.clone(), t, false)),
        Some(TagOp::Clear) => acts.push(Act::SetPrimary(it.id.clone(), None)),
        None => {}
    }
}

/// text_edit_with_menu 的结果
struct MenuEdit {
    resp: egui::Response,
    /// 右键菜单是否开着（点菜单项时编辑框会暂时失去焦点，调用方据此别把它当成“点到别处”）
    menu_open: bool,
    /// 菜单里选了「保存」或「取消」，交给调用方处理；其余操作已在这里执行完
    op: Option<EditOp>,
}

/// 带右键菜单的编辑框（卡片编辑和「编辑原文」共用）：
/// - egui 的编辑框在任意鼠标键按下时都会把光标移到点击处，右键也不例外，选区就没了。
///   右键按下时先记住选区，编辑框处理完再放回去。
/// - 右键菜单：撤销、重做、剪切、复制、粘贴、全选，以及调用方指定文字的「保存」「取消」。
fn text_edit_with_menu(
    ui: &mut Ui,
    id: Id,
    buf: &mut String,
    save: (&str, &str),
    cancel: (&str, &str),
    make: impl FnOnce(&mut String) -> TextEdit<'_>,
    layouter: Option<&mut dyn FnMut(&Ui, &str, f32) -> Arc<egui::Galley>>,
) -> MenuEdit {
    // Ctrl+/：选中的文字加上 / 去掉 %%…%%。要在编辑框处理按键之前吃掉
    if ui.memory(|m| m.has_focus(id))
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Slash))
    {
        apply_edit_op(ui.ctx(), id, buf, &EditOp::Comment);
    }
    // Ctrl+E：选中的文字加上 / 去掉 ==…==
    if ui.memory(|m| m.has_focus(id)) && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::E)) {
        apply_edit_op(ui.ctx(), id, buf, &EditOp::Mark);
    }
    let right_press = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
    let saved_range = if right_press {
        egui::text_edit::TextEditState::load(ui.ctx(), id).and_then(|s| s.cursor.char_range())
    } else {
        None
    };
    let resp = ui
        .scope(|ui| {
            input_visuals(ui);
            let mut te = make(buf).id(id);
            if let Some(l) = layouter {
                te = te.layouter(l);
            }
            ui.add(te)
        })
        .inner;
    if let Some(r) = saved_range {
        if resp.contains_pointer() {
            if let Some(mut st) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
                st.cursor.set_char_range(Some(r));
                st.store(ui.ctx(), id);
            }
            // 这一帧画出来的是被移动过的光标，马上再画一帧显示恢复后的选区
            ui.ctx().request_repaint();
        }
    }

    let (has_sel, can_undo, can_redo) = {
        let st = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        let range = st.cursor.char_range();
        let cur = (range.unwrap_or_default(), buf.clone());
        let undoer = st.undoer();
        (
            range.is_some_and(|r| r.primary.index != r.secondary.index),
            undoer.has_undo(&cur),
            undoer.has_redo(&cur),
        )
    };
    let mut op = None;
    resp.context_menu(|ui| op = edit_menu(ui, has_sel, can_undo, can_redo, save, cancel));
    // 注意：egui 0.31 的 context_menu() 只在菜单关闭的那一帧返回 Some，
    // 不能用它判断菜单是否开着，要用 context_menu_opened()
    let menu_open = resp.context_menu_opened();

    let op = match op {
        Some(EditOp::Save) => Some(EditOp::Save),
        Some(EditOp::Cancel) => Some(EditOp::Cancel),
        Some(other) => {
            apply_edit_op(ui.ctx(), id, buf, &other);
            ui.memory_mut(|m| m.request_focus(id));
            None
        }
        None => None,
    };
    MenuEdit { resp, menu_open, op }
}

/// 编辑框右键菜单里的操作
enum EditOp {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// 选中的文字加上 / 去掉 %%…%%（复制时跳过）
    Comment,
    /// 选中的文字加上 / 去掉 ==…==（部分复制时只复制这些）
    Mark,
    Save,
    Cancel,
}

fn menu_item(ui: &mut Ui, enabled: bool, label: &str, keys: &str) -> bool {
    let clicked = ui
        .add_enabled(enabled, egui::Button::new(label).shortcut_text(keys))
        .clicked();
    if clicked {
        ui.close_menu();
    }
    clicked
}

/// 编辑框的右键菜单，返回选中的操作
fn edit_menu(
    ui: &mut Ui,
    has_sel: bool,
    can_undo: bool,
    can_redo: bool,
    save: (&str, &str),
    cancel: (&str, &str),
) -> Option<EditOp> {
    let mut op = None;
    if menu_item(ui, can_undo, "撤销", "Ctrl+Z") {
        op = Some(EditOp::Undo);
    }
    if menu_item(ui, can_redo, "重做", "Ctrl+Y") {
        op = Some(EditOp::Redo);
    }
    ui.separator();
    if menu_item(ui, has_sel, "剪切", "Ctrl+X") {
        op = Some(EditOp::Cut);
    }
    if menu_item(ui, has_sel, "复制", "Ctrl+C") {
        op = Some(EditOp::Copy);
    }
    if menu_item(ui, true, "粘贴", "Ctrl+V") {
        op = Some(EditOp::Paste);
    }
    if menu_item(ui, true, "全选", "Ctrl+A") {
        op = Some(EditOp::SelectAll);
    }
    if menu_item(ui, has_sel, "忽略复制 / 取消", "Ctrl+/") {
        op = Some(EditOp::Comment);
    }
    if menu_item(ui, has_sel, "复制选中 / 取消", "Ctrl+E") {
        op = Some(EditOp::Mark);
    }
    ui.separator();
    if menu_item(ui, true, save.0, save.1) {
        op = Some(EditOp::Save);
    }
    if menu_item(ui, true, cancel.0, cancel.1) {
        op = Some(EditOp::Cancel);
    }
    op
}

/// 对编辑框执行剪切 / 复制 / 粘贴等操作。
/// 光标、选区和撤销记录都存在 egui 的编辑框状态里，改完写回去，Ctrl+Z 也能撤销这些操作。
fn apply_edit_op(ctx: &egui::Context, edit_id: Id, buf: &mut String, op: &EditOp) {
    use egui::text::{CCursor, CCursorRange};
    let mut st = egui::text_edit::TextEditState::load(ctx, edit_id).unwrap_or_default();
    let n = buf.chars().count();
    let range = st.cursor.char_range().unwrap_or(CCursorRange::one(CCursor::new(n)));
    let (a, b) = {
        let (x, y) = (range.primary.index.min(n), range.secondary.index.min(n));
        (x.min(y), x.max(y))
    };
    let byte = |s: &str, ci: usize| s.char_indices().nth(ci).map_or(s.len(), |(i, _)| i);
    // 用 with 替换选中的部分，返回新的光标位置
    let replace = |buf: &mut String, with: &str| {
        let (ba, bb) = (byte(buf, a), byte(buf, b));
        buf.replace_range(ba..bb, with);
        CCursorRange::one(CCursor::new(a + with.chars().count()))
    };
    let mut undoer = st.undoer();
    let before = (range, buf.clone());
    let new_range = match op {
        EditOp::Undo => undoer.undo(&before).cloned().map(|(r, t)| {
            *buf = t;
            r
        }),
        EditOp::Redo => undoer.redo(&before).cloned().map(|(r, t)| {
            *buf = t;
            r
        }),
        EditOp::Copy => {
            if a < b {
                ctx.copy_text(buf[byte(buf, a)..byte(buf, b)].to_string());
            }
            Some(range)
        }
        EditOp::Cut if a < b => {
            ctx.copy_text(buf[byte(buf, a)..byte(buf, b)].to_string());
            undoer.add_undo(&before);
            Some(replace(buf, ""))
        }
        EditOp::Paste => match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(t) => {
                let t = t.replace("\r\n", "\n").replace('\r', "\n");
                undoer.add_undo(&before);
                Some(replace(buf, &t))
            }
            Err(_) => None,
        },
        EditOp::SelectAll => Some(CCursorRange::two(CCursor::new(0), CCursor::new(n))),
        EditOp::Comment | EditOp::Mark if a < b => {
            let m = if matches!(op, EditOp::Comment) { "%%" } else { "==" };
            let (ba, bb) = (byte(buf, a), byte(buf, b));
            let sel = buf[ba..bb].to_string();
            undoer.add_undo(&before);
            if sel.len() >= 4 && sel.starts_with(m) && sel.ends_with(m) {
                // 选中的连同记号：去掉记号
                let inner = &sel[2..sel.len() - 2];
                let r = replace(buf, inner);
                Some(CCursorRange::two(CCursor::new(a), r.primary))
            } else if buf[..ba].ends_with(m) && buf[bb..].starts_with(m) {
                // 选中的是记号里面的文字：去掉两边的记号
                buf.replace_range(bb..bb + 2, "");
                buf.replace_range(ba - 2..ba, "");
                Some(CCursorRange::two(CCursor::new(a - 2), CCursor::new(b - 2)))
            } else {
                let r = replace(buf, &format!("{m}{sel}{m}"));
                Some(CCursorRange::two(CCursor::new(a), r.primary))
            }
        }
        EditOp::Cut | EditOp::Comment | EditOp::Mark | EditOp::Save | EditOp::Cancel => None,
    };
    if let Some(r) = new_range {
        st.cursor.set_char_range(Some(r));
    }
    st.set_undoer(undoer);
    st.store(ctx, edit_id);
}

#[allow(clippy::too_many_arguments)]
fn item_row_content(
    ui: &mut Ui,
    it: &Item,
    place: u8,
    show_file: bool,
    editing: &mut Option<(String, String, u8)>,
    edit_focus: &mut bool,
    acts: &mut Vec<Act>,
) -> Option<String> {
    // 鼠标所在的网址（卡片据此处理 Ctrl+单击）
    let mut hovered_url = None;
    ui.horizontal(|ui| {
        let mut done = it.done;
        let changed = clear_checkbox(ui, &mut done, "").changed();
        if changed {
            acts.push(Act::SetDone(it.id.clone(), done));
        }

        let is_editing = editing.as_ref().is_some_and(|(id, _, p)| *id == it.id && *p == place);
        if is_editing {
            ui.vertical(|ui| {
                let edit_id = Id::new(("edit", it.id.as_str(), place));
                // 多行编辑：回车保存，Shift+回车换行。
                // 回车要在编辑框处理之前吃掉，否则它会先插入一个换行
                let enter = ui.memory(|m| m.has_focus(edit_id))
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
                let (_, buf, _) = editing.as_mut().unwrap();
                let MenuEdit { resp, menu_open, op } = text_edit_with_menu(
                    ui,
                    edit_id,
                    buf,
                    ("保存", "回车"),
                    ("取消编辑", "Esc"),
                    |buf| TextEdit::multiline(buf).desired_rows(1).desired_width(f32::INFINITY),
                    None,
                );
                if *edit_focus {
                    resp.request_focus();
                    *edit_focus = false;
                }
                // 点菜单项时编辑框会暂时失去焦点，那不算“点到别处”，不能当成保存。
                // 菜单是被点外面关掉的，就按点到别处处理（保存）。
                let menu_key = edit_id.with("menu_open");
                let menu_was_open = ui.data(|d| d.get_temp::<bool>(menu_key)).unwrap_or(false);
                ui.data_mut(|d| d.insert_temp(menu_key, menu_open));
                match op {
                    Some(EditOp::Save) => acts.push(Act::CommitEdit),
                    Some(_) => *editing = None,
                    None if enter => {
                        ui.memory_mut(|m| m.surrender_focus(edit_id));
                        acts.push(Act::CommitEdit);
                    }
                    None if !menu_open
                        && (resp.lost_focus() || (menu_was_open && !resp.has_focus())) =>
                    {
                        if ui.input(|i| i.key_pressed(Key::Escape)) {
                            *editing = None;
                        } else {
                            acts.push(Act::CommitEdit);
                        }
                    }
                    None => {}
                }
                ui.label(
                    RichText::new("回车保存 · Shift+回车换行 · Esc 取消 · 右键更多")
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            });
        } else {
            ui.vertical(|ui| {
                // 卡片上只显示一行：多行只显示首行，过长截断成 …；
                // 双击或右键「编辑」时才显示全部内容
                let body = it.body();
                let mut lines = body.lines();
                let first = lines.next().unwrap_or("").to_string();
                let more = lines.next().is_some();
                // %%备注%% 显示成灰色（复制内容时不复制）；网址加下划线
                let v = ui.visuals();
                let (normal, weak, link) = (v.text_color(), v.weak_text_color(), v.hyperlink_color);
                let font = egui::FontSelection::Default.resolve(ui.style());
                let mut job = egui::text::LayoutJob::default();
                let mut add = |s: &str, comment: bool| {
                    let color = if comment || it.done { weak } else { normal };
                    let mut f = egui::text::TextFormat::simple(font.clone(), color);
                    if it.done && !comment {
                        f.strikethrough = Stroke::new(1.0_f32, color);
                    }
                    job.append(s, 0.0, f);
                };
                if body.is_empty() {
                    add("（空）", false);
                } else {
                    for (r, comment) in split_comments(&first) {
                        add(&first[r], comment);
                    }
                    if more {
                        add(" …", false);
                    }
                }
                if !body.is_empty() {
                    let bg = mark_bg(ui.visuals().dark_mode);
                    restyle(&mut job, &find_marks(&first), |f| f.background = bg);
                }
                let urls = if body.is_empty() { Vec::new() } else { find_urls(&first) };
                restyle(&mut job, &urls, |f| {
                    if f.color != weak {
                        f.color = link;
                    }
                    f.underline = Stroke::new(1.0_f32, f.color);
                });
                let text = job.text.clone();
                let (pos, galley, lresp) = Label::new(job).truncate().layout_in_ui(ui);
                lresp.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, galley.text())
                });
                ui.painter().galley(pos, galley.clone(), normal);
                // 鼠标在网址上：提示，按住 Ctrl 时显示手形
                if let Some(p) = ui.ctx().pointer_hover_pos().filter(|p| lresp.rect.contains(*p)) {
                    let ci = galley.cursor_from_pos(p - pos).ccursor.index;
                    let bi = text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b);
                    // cursor_from_pos 取最近的字符边界，要求落在网址里面
                    if let Some(r) = urls.iter().find(|r| r.start <= bi && bi < r.end) {
                        let u = text[r.clone()].to_string();
                        if ui.input(|i| i.modifiers.command) {
                            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                        }
                        lresp.on_hover_text(format!("{u}\nCtrl+单击打开"));
                        hovered_url = Some(u);
                    }
                }

                let tags = it.tags();
                if !tags.is_empty() || show_file {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for t in &tags {
                            tag_chip(ui, t);
                        }
                        if show_file {
                            ui.label(
                                RichText::new(file_label(&it.file))
                                    .small()
                                    .color(ui.visuals().weak_text_color()),
                            );
                        }
                    });
                }
            });
        }
    });
    hovered_url
}

/// 看得清的勾选框：默认的勾选框底色和背景（卡片、面板）几乎一样，看不清，
/// 改成白底（深色模式近黑）+ 明显的边框，悬停时边框用强调色加粗。只影响这一个勾选框。
fn clear_checkbox(ui: &mut Ui, checked: &mut bool, text: &str) -> egui::Response {
    ui.scope(|ui| {
        let v = ui.visuals_mut();
        let (fill, edge) = if let Some(c) = warm(v) {
            (c.input, c.line_strong)
        } else if v.dark_mode {
            (Color32::from_gray(18), Color32::from_gray(150))
        } else {
            (Color32::WHITE, Color32::from_gray(140))
        };
        let accent = v.selection.stroke.color;
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
            w.bg_fill = fill;
            w.bg_stroke = Stroke::new(1.0_f32, edge);
        }
        v.widgets.hovered.bg_stroke = Stroke::new(1.5_f32, accent);
        v.widgets.active.bg_stroke = Stroke::new(1.5_f32, accent);
        ui.checkbox(checked, text)
    })
    .inner
}

/// 单行输入框的上下内边距（egui 默认 2，加大一些更明显、更好点中）
const INPUT_PAD_Y: i8 = 5;

/// 单行输入框的内边距（左右 8：有了边框后，文字贴着边框会显得挤）
fn input_margin() -> egui::Margin {
    egui::Margin::symmetric(8, INPUT_PAD_Y)
}

/// 输入框统一样式：平时有一圈中灰细边框，悬停加深，输入时强调色加粗；圆角跟着 UI设置。
/// 在 ui.scope 里调用，只影响这一个输入框，不影响按钮等其他控件。
fn input_visuals(ui: &mut Ui) {
    let r = uip(ui).radius;
    let v = ui.visuals_mut();
    let (edge, edge_hover) = if let Some(c) = warm(v) {
        (c.line, c.line_strong)
    } else if v.dark_mode {
        (Color32::from_gray(95), Color32::from_gray(150))
    } else {
        (Color32::from_gray(175), Color32::from_gray(115))
    };
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.corner_radius = r.into();
        // egui 默认悬停时外扩 1px，有了常驻边框会看到输入框“跳”一下
        w.expansion = 0.0;
    }
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, edge);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, edge_hover);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, edge_hover);
    v.selection.stroke.width = 1.5;
}

/// 设置里的数值项：「−」拖动条「+」。− / + 每点一下改一个最小步长，到头时变灰。
fn stepper(ui: &mut Ui, v: &mut f32, min: f32, max: f32, step: f32, suffix: &str) {
    ui.horizontal(|ui| {
        let side = egui::vec2(ui.spacing().interact_size.y, ui.spacing().interact_size.y);
        let snap = |x: f32| ((x / step).round() * step).clamp(min, max);
        if ui
            .add_enabled(*v > min, egui::Button::new("-").min_size(side))
            .on_hover_text(format!("减 {step}"))
            .clicked()
        {
            *v = snap(*v - step);
        }
        ui.add(egui::Slider::new(v, min..=max).step_by(step as f64).suffix(suffix));
        if ui
            .add_enabled(*v < max, egui::Button::new("+").min_size(side))
            .on_hover_text(format!("加 {step}"))
            .clicked()
        {
            *v = snap(*v + step);
        }
    });
}

/// 输入框旁的「×」清空按钮：固定显示，和卡片一样的圆角底色，悬停加深，整块范围内点击都有效。
/// 输入框没字时 × 变浅（点了也没东西可清）。
fn clear_button(ui: &mut Ui, has_text: bool) -> egui::Response {
    // 和单行输入框一样高（文字高度 + 输入框上下内边距）
    let side = ui.text_style_height(&egui::TextStyle::Body) + 2.0 * INPUT_PAD_Y as f32;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "×"));
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let hovered = resp.hovered();
        ui.painter().rect_filled(rect, uip(ui).radius, block_fill(v, hovered, false));
        let color = if !has_text {
            v.weak_text_color()
        } else if hovered {
            v.strong_text_color()
        } else {
            v.text_color()
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::TextStyle::Body.resolve(ui.style()),
            color,
        );
    }
    resp.on_hover_cursor(CursorIcon::PointingHand)
}

/// 左栏的一个导航块：整块可点击，也是拖放目标。高度是普通一行的若干倍（设置里调，默认 2 倍），
/// 名字过长时截断，数字靠右。
fn nav_block(
    ui: &mut Ui,
    salt: impl std::hash::Hash,
    selected: bool,
    indent: f32,
    text: RichText,
    count: usize,
) -> egui::Response {
    // 普通一行（selectable_label）的高度
    let row_h = (ui.text_style_height(&egui::TextStyle::Body) + 2.0 * ui.spacing().button_padding.y)
        .max(ui.spacing().interact_size.y);
    let block = ui.scope_builder(egui::UiBuilder::new().id_salt(salt).sense(Sense::click()), |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        let hovered = ui.response().contains_pointer();
        let v = ui.visuals();
        let fill = if selected { v.selection.bg_fill } else { block_fill(v, hovered, false) };
        let stroke = if selected {
            v.selection.stroke.color
        } else if let Some(c) = warm(v) {
            c.card_line
        } else {
            Color32::TRANSPARENT
        };
        egui::Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0_f32, stroke))
            .corner_radius(uip(ui).radius)
            .inner_margin(egui::Margin::symmetric(8, 0))
            .show(ui, |ui| {
                // 减去上下描边，整块外框正好是设置里的行数高
                let size = egui::vec2(ui.available_width(), row_h * uip(ui).nav_rows - 2.0);
                ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_min_size(size);
                    ui.add_space(indent);
                    let num = count.to_string();
                    let weak = ui.visuals().weak_text_color();
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    let num_w = ui.fonts(|f| f.layout_no_wrap(num.clone(), font, weak).size().x);
                    // 名字区域 = 剩余宽度 − 数字 − 间距，再留 1px 余量：
                    // 加起来一旦超过侧栏宽度，侧栏会被逐帧撑大
                    let name_w = (ui.available_width() - num_w - ui.spacing().item_spacing.x - 1.0).max(0.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(name_w, size.y),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(name_w);
                            ui.add(Label::new(text).truncate());
                        },
                    );
                    ui.label(RichText::new(num).color(weak));
                });
            });
    });
    block.response
}

/// 「刚刚」「3 分钟前」「2 天前」：不需要时区，也不用引入时间库
fn time_ago(ts: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(ts);
    let d = now.saturating_sub(ts);
    match d {
        0..=59 => "刚刚".to_string(),
        60..=3599 => format!("{} 分钟前", d / 60),
        3600..=86399 => format!("{} 小时前", d / 3600),
        _ => format!("{} 天前", d / 86400),
    }
}

/// 放置目标高亮
fn drop_highlight(ui: &Ui, rect: Rect) {
    let c = ui.visuals().selection.stroke.color;
    ui.painter()
        .rect_stroke(rect.expand(1.0), 4.0, Stroke::new(1.5_f32, c), egui::StrokeKind::Inside);
}

fn insert_line(ui: &Ui, rect: Rect, after: bool) {
    let y = if after { rect.bottom() + 2.0 } else { rect.top() - 2.0 };
    let c = ui.visuals().selection.stroke.color;
    ui.painter().hline(rect.x_range(), y, Stroke::new(2.0_f32, c));
}

/// 两段切换按钮，返回是否切到了第二项
/// 左栏标签前面折叠箭头占的宽度
const FOLD_W: f32 = 16.0;

/// 折叠箭头：展开时朝下，折叠时朝右（自己画三角，不依赖字体里有没有这个符号）
fn fold_arrow(painter: &egui::Painter, c: egui::Pos2, collapsed: bool, color: Color32) {
    let s = 4.0;
    let pts = if collapsed {
        vec![c + egui::vec2(-s * 0.6, -s), c + egui::vec2(s * 0.9, 0.0), c + egui::vec2(-s * 0.6, s)]
    } else {
        vec![c + egui::vec2(-s, -s * 0.6), c + egui::vec2(s, -s * 0.6), c + egui::vec2(0.0, s * 0.9)]
    };
    painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// 两种显示方式二选一：每个选项一个圆角框，中间拉开间距；选中的用强调色底色和边框
fn segmented(ui: &mut Ui, second: &mut bool, a: &str, b: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.spacing_mut().button_padding = egui::vec2(10.0, 3.0);
        let r = uip(ui).radius.max(4.0);
        let edge = if let Some(c) = warm(ui.visuals()) {
            c.line
        } else if ui.visuals().dark_mode {
            Color32::from_gray(95)
        } else {
            Color32::from_gray(175)
        };
        let accent = ui.visuals().selection.stroke.color;
        for (i, label) in [a, b].into_iter().enumerate() {
            let on = (i == 1) == *second;
            let stroke = Stroke::new(1.0_f32, if on { accent } else { edge });
            let fill = if on { ui.visuals().selection.bg_fill } else { Color32::TRANSPARENT };
            let resp = ui.add(egui::Button::new(label).fill(fill).stroke(stroke).corner_radius(r));
            if resp.clicked() && !on {
                *second = i == 1;
                changed = true;
            }
        }
    });
    changed
}

// ---------- 主界面 ----------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

impl App {
    fn ui(&mut self, ctx: &egui::Context) {
        let mut acts: Vec<Act> = Vec::new();
        let shift = ctx.input(|i| i.modifiers.shift);
        if !self.ui_applied {
            apply_ui_prefs(ctx, &self.store.prefs.ui);
            self.ui_applied = true;
        }
        ctx.data_mut(|d| d.insert_temp(Id::new("ui_prefs"), self.store.prefs.ui.clone()));
        self.handle_keys(ctx, &mut acts);
        self.handle_file_drop(ctx);

        // 顶部：快速添加 + 搜索
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            let target = self.target_file();
            // 左侧选中了某个标签时，新条目自动带上它
            let auto_tag = match &self.sel {
                Sel::Tag(t) => Some(t.clone()),
                _ => None,
            };
            ui.horizontal(|ui| {
                ui.label("＋");
                self.tag_picker(ui);
                if clear_button(ui, !self.quick.is_empty()).on_hover_text("清空输入").clicked() {
                    self.quick.clear();
                    self.quick_focus = true;
                }
                let quick_id = Id::new("quick_add");
                let resp = ui
                    .scope(|ui| {
                        input_visuals(ui);
                        ui.add(
                            TextEdit::singleline(&mut self.quick)
                                .id(quick_id)
                                .margin(input_margin())
                                .hint_text("快速添加，# 加标签，回车保存")
                                .desired_width(ui.available_width() - 200.0),
                        )
                    })
                    .inner;
                if self.quick_focus {
                    resp.request_focus();
                    self.quick_focus = false;
                    // 光标移到末尾（从标签菜单回来时，文字已经变了）
                    if let Some(mut st) = egui::text_edit::TextEditState::load(ui.ctx(), quick_id) {
                        let end = egui::text::CCursor::new(self.quick.chars().count());
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                        st.store(ui.ctx(), quick_id);
                    }
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    if !self.quick.trim().is_empty() {
                        let mut text = std::mem::take(&mut self.quick);
                        if let Some(t) = &auto_tag {
                            text = store::ensure_tag(&text, t);
                        }
                        self.store.add(&text, &target);
                    }
                    resp.request_focus();
                }
                ui.label(
                    RichText::new(match &auto_tag {
                        Some(t) => format!("→ {} #{t}", file_label(&target)),
                        None => format!("→ {}", file_label(&target)),
                    })
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            });
            ui.horizontal(|ui| {
                ui.label("🔍");
                let search_id = Id::new("search");
                ui.scope(|ui| {
                    input_visuals(ui);
                    ui.add(
                        TextEdit::singleline(&mut self.search)
                            .id(search_id)
                            .margin(input_margin())
                            .hint_text("搜索")
                            .desired_width(220.0),
                    );
                });
                if clear_button(ui, !self.search.is_empty()).on_hover_text("清空搜索").clicked() {
                    self.search.clear();
                    ui.memory_mut(|m| m.request_focus(search_id));
                }
                if clear_checkbox(ui, &mut self.store.prefs.hide_done, "隐藏已完成").changed() {
                    self.store.save_state();
                }
                if ui.add(egui::Button::new("UI设置").selected(self.settings_open)).on_hover_text("外观设置").clicked() {
                    self.settings_open = !self.settings_open;
                }
                if ui.button("刷新").on_hover_text("重新读取磁盘上的文件").clicked() {
                    self.store.reload();
                    if let Some(r) = &mut self.raw {
                        if !r.dirty() {
                            *r = RawEdit::new(r.path.clone(), self.store.read_raw(&r.path));
                        }
                    }
                    self.editing = None;
                }
                ui.label(
                    RichText::new(self.store.root.display().to_string())
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            });
            if let Some(err) = self.store.error.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, err);
                    if ui.small_button("✕").clicked() {
                        self.store.error = None;
                    }
                });
            }
            ui.add_space(4.0);
        });

        // 左侧：文档 / 按标签
        // 左栏里点了哪里，画完再切换（画的过程中还借着 store 统计数量）
        let mut go: Option<Sel> = None;
        egui::SidePanel::left("left").resizable(true).default_width(210.0).show(ctx, |ui| {
            ui.add_space(6.0);
            let mut by_tag = self.store.prefs.left_by_tag;
            if segmented(ui, &mut by_tag, "文档", "按标签") {
                self.store.prefs.left_by_tag = by_tag;
                self.store.save_state();
                self.goto(Sel::All);
            }
            ui.separator();

            ScrollArea::vertical().id_salt("left_scroll").auto_shrink([false, false]).show(ui, |ui| {
                let open = |pred: &dyn Fn(&Item) -> bool| {
                    self.store.items.values().filter(|it| !it.done && pred(it)).count()
                };
                ui.spacing_mut().item_spacing.y = uip(ui).gap;
                let total_open = open(&|_: &Item| true);
                let all = nav_block(ui, "nav_all", self.sel == Sel::All, 0.0, RichText::new("全部"), total_open);
                if all.clicked() {
                    go = Some(Sel::All);
                }

                if !by_tag {
                    // 文档视图（所有文档都在素材库这一层，没有子文件夹）
                    // 固定文档（收件箱、保管箱）不在这里列，放到回收站下面
                    let docs: Vec<PathBuf> = self
                        .store
                        .docs
                        .iter()
                        .map(|d| d.path.clone())
                        .filter(|p| !is_fixed_doc(p))
                        .collect();
                    for path in docs {
                        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        self.file_block(ui, &path, 0.0, format!("📄 {name}"), &mut acts);
                    }

                    ui.add_space(10.0);
                    // 先放按钮、再让输入框占满剩余宽度：若按估算值预留按钮宽度，
                    // 一旦估小，这一行就比侧栏宽，侧栏会被逐帧撑大
                    let row = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
                    ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let add = ui.button("＋").clicked();
                        let r = ui
                            .scope(|ui| {
                                input_visuals(ui);
                                ui.add(
                                    TextEdit::singleline(&mut self.new_doc)
                                        .margin(input_margin())
                                        .hint_text("新建文档")
                                        .desired_width(ui.available_width()),
                                )
                            })
                            .inner;
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if add || enter {
                            let name = std::mem::take(&mut self.new_doc);
                            if let Some(p) = self.store.create_doc(&name) {
                                self.goto(Sel::File(p));
                            }
                        }
                    });

                    ui.add_space(10.0);
                    let trash = nav_block(
                        ui,
                        "nav_trash",
                        self.sel == Sel::Trash,
                        0.0,
                        RichText::new("🗑 回收站"),
                        self.store.trash.len(),
                    );
                    if trash.clicked() {
                        self.goto(Sel::Trash);
                    }
                    for (f, icon) in FIXED_DOCS.iter().zip(["📥", "🔒"]) {
                        let path = PathBuf::from(f);
                        let name = f.trim_end_matches(".md");
                        self.file_block(ui, &path, 0.0, format!("{icon} {name}"), &mut acts);
                    }
                } else {
                    // 分类视图
                    let n_untagged = open(&|it: &Item| it.tags().is_empty());
                    let resp = nav_block(
                        ui,
                        "nav_untagged",
                        self.sel == Sel::Untagged,
                        0.0,
                        RichText::new("无标签"),
                        n_untagged,
                    );
                    if resp.clicked() {
                        go = Some(Sel::Untagged);
                    }
                    if resp.dnd_hover_payload::<DragItem>().is_some() {
                        drop_highlight(ui, resp.rect);
                    }
                    if let Some(p) = resp.dnd_release_payload::<DragItem>() {
                        acts.push(Act::SetPrimary(p.0.clone(), None));
                    }

                    let tags = self.store.all_tags();
                    let mut toggle: Option<String> = None;
                    for tag in &tags {
                        let tag = tag.clone();
                        // 祖先标签折叠了就不显示
                        let hidden = self.store.prefs.collapsed_tags.iter().any(|c| tag.starts_with(&format!("{c}/")));
                        if hidden {
                            continue;
                        }
                        let depth = tag.matches('/').count();
                        let short = tag.rsplit('/').next().unwrap_or(&tag).to_string();
                        let n = open(&|it: &Item| it.has_tag(&tag));
                        let selected = self.sel == Sel::Tag(tag.clone());
                        let (_, fg) = tag_colors(&tag, ui.visuals().dark_mode);
                        let has_children = tags.iter().any(|t| t.starts_with(&format!("{tag}/")));
                        let collapsed = self.store.prefs.collapsed_tags.contains(&tag);
                        // 每层缩进 14，前面再留 16 放折叠箭头（没有子标签的也留，文字对齐）
                        let indent = depth as f32 * 14.0;
                        let resp = nav_block(
                            ui,
                            ("nav_tag", &tag),
                            selected,
                            indent + FOLD_W,
                            RichText::new(format!("# {short}")).color(fg),
                            n,
                        );
                        if has_children {
                            // 箭头区域盖在块上面，点它只折叠 / 展开，不选中
                            let x0 = resp.rect.left() + 9.0 + indent;
                            let arrow = Rect::from_x_y_ranges(x0..=x0 + FOLD_W, resp.rect.y_range());
                            let ar = ui
                                .interact(arrow, Id::new(("fold", &tag)), Sense::click())
                                .on_hover_text(if collapsed { "展开子标签" } else { "折叠子标签" });
                            let color = if ar.hovered() {
                                ui.visuals().strong_text_color()
                            } else {
                                ui.visuals().weak_text_color()
                            };
                            fold_arrow(ui.painter(), arrow.center(), collapsed, color);
                            if ar.clicked() {
                                toggle = Some(tag.clone());
                            }
                        }
                        if resp.clicked() {
                            go = Some(Sel::Tag(tag.clone()));
                        }
                        // 双击块：折叠 / 展开子标签
                        if has_children && resp.double_clicked() {
                            toggle = Some(tag.clone());
                        }
                        if resp.dnd_hover_payload::<DragItem>().is_some() {
                            drop_highlight(ui, resp.rect);
                        }
                        if let Some(p) = resp.dnd_release_payload::<DragItem>() {
                            acts.push(Act::AddTag(p.0.clone(), tag.clone(), shift));
                        }
                    }
                    if let Some(t) = toggle {
                        let c = &mut self.store.prefs.collapsed_tags;
                        if let Some(i) = c.iter().position(|x| *x == t) {
                            c.remove(i);
                        } else {
                            c.push(t);
                        }
                        self.store.save_state();
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("拖到标签上 = 追加标签\n按住 Shift 拖 = 替换标签")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                }
            });
        });

        if let Some(sel) = go {
            self.goto(sel);
        }

        // 右侧：TODO。「手动排序」= 没完成的，可以拖动排序；「已完成」= 勾掉的
        egui::SidePanel::right("right").resizable(true).default_width(280.0).show(ctx, |ui| {
            ui.add_space(6.0);
            let mut show_done = self.store.prefs.right_done;
            let changed = ui
                .horizontal(|ui| {
                    ui.strong("TODO：");
                    segmented(ui, &mut show_done, "手动排序", "已完成")
                })
                .inner;
            if changed {
                self.store.prefs.right_done = show_done;
                self.store.save_state();
            }
            ui.separator();

            let pointer_y = ctx.pointer_interact_pos().map(|p| p.y).unwrap_or(0.0);
            // 块与块之间留 4px，放置区上下各扩一半，正好接上
            let block_gap = self.store.prefs.ui.gap;
            let gap = block_gap / 2.0;

            // 看不见也要画的：正在编辑的、正在拖动的
            let drag_id = egui::DragAndDrop::payload::<DragItem>(ctx).map(|p| p.0.clone());
            let edit_id = self.editing.as_ref().filter(|e| e.2 == 1).map(|e| e.0.clone());
            let keep_right = [edit_id, drag_id];
            let selected_id = self.selected.as_ref().map(|s| s.0.clone());

            // 右栏只看完成状态和搜索，不受「隐藏已完成」影响（那是给中间栏用的）
            let q = self.search.trim().to_lowercase();
            let ids: Vec<String> = self
                .store
                .order
                .iter()
                .filter(|id| {
                    self.store.items.get(*id).is_some_and(|it| {
                        it.done == show_done
                            && self.store.in_todo(&it.file)
                            && (q.is_empty() || it.text.to_lowercase().contains(&q))
                    })
                })
                .cloned()
                .collect();

            ScrollArea::vertical().id_salt("right_scroll").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
                ui.spacing_mut().item_spacing.y = block_gap;
                if ids.is_empty() {
                    ui.add_space(20.0);
                    let tip = if show_done { "还没有已完成的条目" } else { "没有待办。勾掉的条目在「已完成」里" };
                    ui.label(RichText::new(tip).color(ui.visuals().weak_text_color()));
                    return;
                }
                let keep = |i: usize| keep_right.iter().any(|k| k.as_deref() == Some(ids[i].as_str()));
                let store = &self.store;
                let editing = &mut self.editing;
                let edit_focus = &mut self.edit_focus;
                virtual_list(ui, viewport, &mut self.row_h[1], &ids, keep, |ui, i| {
                    let id = &ids[i];
                    let Some(it) = store.items.get(id) else { return };
                    let sel = selected_id.as_deref() == Some(id.as_str());
                    // 只有「手动排序」能拖动；「已完成」里不能拖
                    let rect = ui
                        .push_id(("todo", id.as_str()), |ui| {
                            item_row(ui, it, 1, false, sel, store, editing, edit_focus, &mut acts, !show_done)
                        })
                        .inner
                        .expand2(egui::vec2(0.0, gap));
                    if show_done {
                        return;
                    }
                    let resp = ui.interact(rect, Id::new(("todo_drop", id.as_str())), Sense::hover());
                    let after = pointer_y > rect.center().y;
                    if let Some(p) = resp.dnd_hover_payload::<DragItem>() {
                        if p.0 != *id {
                            insert_line(ui, rect.shrink2(egui::vec2(0.0, gap)), after);
                        }
                    }
                    if let Some(p) = resp.dnd_release_payload::<DragItem>() {
                        if p.0 != *id {
                            acts.push(Act::Reorder(p.0.clone(), id.clone(), after));
                        }
                    }
                });
            });
        });

        // 中间：当前选择下的素材卡片 / 原文编辑
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.sel == Sel::Trash {
                self.trash_view(ui, &mut acts);
                return;
            }
            if let Some(raw) = &mut self.raw {
                let mut save = false;
                // 返回：没改动直接返回，有改动先问
                let mut back = false;
                // 右键菜单「返回（不保存）」：明确不保存，直接返回
                let mut discard = false;
                let dirty = raw.dirty();
                ui.horizontal(|ui| {
                    ui.strong(format!("编辑原文：{}", file_label(&raw.path)));
                    if dirty {
                        ui.label(RichText::new("● 未保存").small().color(ui.visuals().warn_fg_color));
                    }
                    if ui.add_enabled(dirty, egui::Button::new("保存")).on_hover_text("Ctrl+S").clicked() {
                        save = true;
                    }
                    if ui.button("返回").on_hover_text("Esc").clicked() {
                        back = true;
                    }
                    ui.separator();
                    // 编辑框失去焦点后选区还记在它的状态里，所以点按钮时选区还在
                    let raw_id = Id::new("raw_edit");
                    let has_sel = egui::text_edit::TextEditState::load(ui.ctx(), raw_id)
                        .and_then(|s| s.cursor.char_range())
                        .is_some_and(|r| r.primary.index != r.secondary.index);
                    let buttons = [
                        (
                            "忽略复制",
                            "选中文字后点一下：加上 %%…%%，复制时跳过；\n再选中点一下取消。快捷键 Ctrl+/",
                            "Ctrl+/",
                            EditOp::Comment,
                        ),
                        (
                            "复制选中",
                            "选中文字后点一下：加上 ==…==，卡片右键「部分复制」只复制这些；\n再选中点一下取消。快捷键 Ctrl+E",
                            "Ctrl+E",
                            EditOp::Mark,
                        ),
                    ];
                    for (label, tip, key, op) in buttons {
                        let b = ui
                            .add_enabled(has_sel, egui::Button::new(label))
                            .on_hover_text(tip)
                            .on_disabled_hover_text(format!("先选中文字。快捷键 {key}"));
                        if b.clicked() {
                            apply_edit_op(ui.ctx(), raw_id, &mut raw.buf, &op);
                            raw.recount();
                            ui.ctx().request_repaint();
                            ui.memory_mut(|m| m.request_focus(raw_id));
                        }
                    }
                });
                ui.separator();
                // 底部状态栏先留出高度，编辑区铺满剩下的
                let status_h = ui.text_style_height(&egui::TextStyle::Small) + ui.spacing().item_spacing.y * 2.0;
                let edit_h = (ui.available_height() - status_h).max(60.0);
                let mut menu_open = false;
                ui.allocate_ui(egui::vec2(ui.available_width(), edit_h), |ui| {
                    ScrollArea::vertical().id_salt("raw_scroll").auto_shrink([false, false]).show(ui, |ui| {
                        let row = ui.text_style_height(&egui::TextStyle::Body);
                        let rows = ((edit_h - 24.0) / row).floor().max(3.0) as usize;
                        let mut layouter = |ui: &Ui, text: &str, wrap: f32| {
                            let style = HiStyle::of(ui);
                            let mut job = ui.ctx().memory_mut(|m| {
                                m.caches.cache::<HiCache>().get((&style, text))
                            });
                            job.wrap.max_width = wrap;
                            ui.fonts(|f| f.layout_job(job))
                        };
                        let r = text_edit_with_menu(
                            ui,
                            Id::new("raw_edit"),
                            &mut raw.buf,
                            ("保存", "Ctrl+S"),
                            ("返回（不保存）", ""),
                            |buf| {
                                TextEdit::multiline(buf)
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(rows)
                                    .margin(egui::Margin::symmetric(10, 8))
                            },
                            Some(&mut layouter),
                        );
                        menu_open = r.menu_open;
                        if r.resp.changed() {
                            raw.recount();
                        }
                        // Ctrl+单击网址：光标已经放到点击处，看它落在哪个网址里
                        if r.resp.clicked() && ui.input(|i| i.modifiers.command) {
                            let ci = egui::text_edit::TextEditState::load(ui.ctx(), Id::new("raw_edit"))
                                .and_then(|s| s.cursor.char_range())
                                .map(|c| c.primary.index);
                            if let Some(ci) = ci {
                                let b = raw.buf.char_indices().nth(ci).map_or(raw.buf.len(), |(b, _)| b);
                                if let Some(r) = find_urls(&raw.buf).into_iter().find(|r| r.start <= b && b <= r.end) {
                                    open_link(ui.ctx(), &raw.buf[r]);
                                }
                            }
                        }
                        match r.op {
                            Some(EditOp::Save) => save = true,
                            Some(_) => discard = true,
                            None => {}
                        }
                    });
                });
                let (lines, chars, items) = raw.stats;
                ui.label(
                    RichText::new(format!("{lines} 行 · {chars} 字 · {items} 条素材"))
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                if self.confirm.is_none() {
                    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::S)) {
                        save = true;
                    }
                    if !menu_open && ctx.input(|i| i.key_pressed(Key::Escape)) {
                        back = true;
                    }
                }
                if save || back || discard {
                    // 状态是画完这一帧才变的，马上再画一帧显示出来
                    ctx.request_repaint();
                }
                if save && dirty {
                    self.save_raw();
                } else if discard || (back && !dirty) {
                    self.raw = None;
                } else if back {
                    self.confirm = Some(Confirm::LeaveRaw(None));
                }
                return;
            }

            if let Some(rd) = &mut self.reading {
                rd.refresh(&self.store);
                let mut edit = false;
                let mut back = false;
                ui.horizontal(|ui| {
                    ui.strong(format!("阅读原文：{}", file_label(&rd.path)));
                    if ui.button("编辑原文").clicked() {
                        edit = true;
                    }
                    if ui.button("返回").on_hover_text("Esc").clicked() {
                        back = true;
                    }
                });
                ui.separator();
                ScrollArea::vertical().id_salt("read_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    let style = HiStyle::of(ui);
                    let mut job = ui.ctx().memory_mut(|m| m.caches.cache::<ReadCache>().get((&style, rd.text.as_str())));
                    let width = ui.available_width();
                    job.wrap.max_width = width;
                    let urls = find_urls(&job.text);
                    let text = job.text.clone();
                    let galley = ui.fonts(|f| f.layout_job(job.clone()));
                    let resp = ui.add(Label::new(job).selectable(true));
                    // 网址：单击打开（阅读时不用按 Ctrl）
                    if let Some(pos) = ui.ctx().pointer_hover_pos().filter(|p| resp.rect.contains(*p)) {
                        let ci = galley.cursor_from_pos(pos - resp.rect.min).ccursor.index;
                        let bi = text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b);
                        if let Some(r) = urls.iter().find(|r| r.start <= bi && bi < r.end) {
                            let u = text[r.clone()].to_string();
                            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                            if resp.clicked() {
                                open_link(ui.ctx(), &u);
                            }
                            resp.on_hover_text(format!("{u}\n单击打开"));
                        }
                    }
                });
                if self.confirm.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) {
                    back = true;
                }
                if edit {
                    let p = rd.path.clone();
                    self.raw = Some(RawEdit::new(p.clone(), self.store.read_raw(&p)));
                    self.reading = None;
                    ctx.request_repaint();
                } else if back {
                    self.reading = None;
                    ctx.request_repaint();
                }
                return;
            }

            let title = match &self.sel {
                Sel::All => "全部".to_string(),
                Sel::File(p) => file_label(p),
                Sel::Tag(t) => format!("# {t}"),
                Sel::Untagged => "无标签".to_string(),
                Sel::Trash => "回收站".to_string(),
            };
            // 改名的结果：Some(Some(新名字)) 保存，Some(None) 取消
            let mut rename_done: Option<Option<String>> = None;
            ui.horizontal(|ui| {
                let renaming = match (&self.sel, &mut self.rename) {
                    (Sel::File(p), Some((rp, buf))) if p == rp => Some(buf),
                    _ => None,
                };
                if let Some(buf) = renaming {
                    let r = ui.add(TextEdit::singleline(buf).desired_width(240.0));
                    if self.rename_focus {
                        r.request_focus();
                        self.rename_focus = false;
                    }
                    if r.lost_focus() {
                        rename_done = Some(if ui.input(|i| i.key_pressed(Key::Escape)) {
                            None
                        } else {
                            Some(buf.clone())
                        });
                    }
                    ui.label(
                        RichText::new("回车保存，Esc 取消")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                    return;
                }

                ui.strong(&title);
                if let Sel::File(p) = &self.sel {
                    if ui.small_button("阅读原文").on_hover_text("整篇只读查看，排版更清爽").clicked() {
                        self.reading = Some(RawRead::open(&self.store, p.clone()));
                    }
                    if ui.small_button("编辑原文").clicked() {
                        self.raw = Some(RawEdit::new(p.clone(), self.store.read_raw(p)));
                    }
                    if !is_fixed_doc(p)
                        && ui.small_button("修改标题").on_hover_text("重命名这个文件").clicked()
                    {
                        let stem = p
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.rename = Some((p.clone(), stem));
                        self.rename_focus = true;
                    }
                }
            });
            if let Some(result) = rename_done {
                if let (Some(name), Some((old, _))) = (result, self.rename.take()) {
                    if let Some(new) = self.store.rename_doc(&old, &name) {
                        self.sel = Sel::File(new);
                    }
                }
                self.rename = None;
            }
            ui.separator();

            let ids: Vec<String> = match &self.sel {
                Sel::File(p) => self.store.doc_items(p),
                _ => self.store.order.clone(),
            };
            let show_file = !matches!(self.sel, Sel::File(_));

            let ids: Vec<String> = ids
                .into_iter()
                .filter(|id| self.store.items.get(id).is_some_and(|it| self.visible(it) && self.sel_match(it)))
                .collect();
            let drag_id = egui::DragAndDrop::payload::<DragItem>(ctx).map(|p| p.0.clone());
            let edit_id = self.editing.as_ref().filter(|e| e.2 == 0).map(|e| e.0.clone());
            let keep_mid = [edit_id, drag_id];
            let selected_id = self.selected.as_ref().map(|s| s.0.clone());

            ScrollArea::vertical().id_salt("mid_scroll").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
                ui.spacing_mut().item_spacing.y = uip(ui).gap;
                let shown = ids.len();
                let keep = |i: usize| keep_mid.iter().any(|k| k.as_deref() == Some(ids[i].as_str()));
                let store = &self.store;
                let editing = &mut self.editing;
                let edit_focus = &mut self.edit_focus;
                virtual_list(ui, viewport, &mut self.row_h[0], &ids, keep, |ui, i| {
                    let id = &ids[i];
                    let Some(it) = store.items.get(id) else { return };
                    let sel = selected_id.as_deref() == Some(id.as_str());
                    ui.push_id(("card", id.as_str()), |ui| {
                        item_row(ui, it, 0, show_file, sel, store, editing, edit_focus, &mut acts, true);
                    });
                });
                if shown == 0 {
                    ui.add_space(20.0);
                    ui.label(
                        RichText::new("这里还没有内容。在顶部输入一条，回车就会出现在这里和右侧待办区。")
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                ui.add_space(12.0);
                ui.label(
                    RichText::new("整块都能操作：单击选中，双击编辑，右键更多，拖到左侧文件 = 移入该文件，拖到右侧 = 排序。\n选中后：空格 完成 / 未完成，回车 编辑，Delete 删除，Esc 取消选中。")
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            });
        });

        // 拖动时跟随鼠标的预览
        if let Some(p) = egui::DragAndDrop::payload::<DragItem>(ctx) {
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            if let (Some(pos), Some(it)) = (ctx.pointer_interact_pos(), self.store.items.get(&p.0)) {
                let text: String = it.body().chars().take(30).collect();
                egui::Area::new(Id::new("drag_preview"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(pos + egui::vec2(14.0, 10.0))
                    .interactable(false)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(text);
                        });
                    });
            }
        }

        self.settings_window(ctx);
        self.confirm_dialog(ctx, &mut acts);
        self.apply(acts);

        // 外观设置：松开鼠标后再写盘，拖动条拖动过程中不反复写
        if self.ui_dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.store.save_state();
            self.ui_dirty = false;
        }
    }

    /// 删除确认框（删条目、删文档、彻底删除共用）：回车 = 确认，Esc / 点框外 = 取消。
    /// 开着时后面的界面点不了，卡片快捷键也不响应。
    fn confirm_dialog(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let Some(c) = self.confirm.clone() else { return };
        if let Confirm::LeaveRaw(next) = c {
            self.leave_raw_dialog(ctx, next);
            return;
        }
        // (标题, 内容, 灰色说明, 确认按钮, 确认后的动作)
        let (title, body, note, danger, act) = match &c {
            Confirm::Item(id) => {
                let Some(it) = self.store.items.get(id) else {
                    self.confirm = None;
                    return;
                };
                // 预览：第一行，过长截断
                let first = it.body().lines().next().unwrap_or("").to_string();
                let mut preview: String = first.chars().take(40).collect();
                if first.chars().count() > 40 || it.body().lines().count() > 1 {
                    preview.push_str(" …");
                }
                if preview.is_empty() {
                    preview = "（空）".into();
                }
                (
                    "删除这一条？".to_string(),
                    preview,
                    format!("所在文件：{}\n删除后不能撤销", file_label(&it.file)),
                    "删除",
                    Act::Delete(id.clone()),
                )
            }
            Confirm::Doc(p) => {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let n = self.store.doc_item_count(p);
                (
                    format!("删除文档「{name}」？"),
                    if n > 0 { format!("其中 {n} 条素材会一起移到回收站。") } else { "文档会移到回收站。".to_string() },
                    "可以在回收站里还原。".to_string(),
                    "删除",
                    Act::DeleteDoc(p.clone()),
                )
            }
            Confirm::LeaveRaw(_) => return,
            Confirm::Purge(files) => {
                let names: Vec<String> = self
                    .store
                    .trash
                    .iter()
                    .filter(|e| files.contains(&e.file))
                    .map(|e| e.original.clone())
                    .collect();
                let mut body = names.iter().take(5).cloned().collect::<Vec<_>>().join("\n");
                if names.len() > 5 {
                    body.push_str(&format!("\n…等 {} 个", names.len()));
                }
                (
                    format!("彻底删除 {} 个文档？", files.len()),
                    body,
                    "彻底删除后无法恢复。".to_string(),
                    "彻底删除",
                    Act::Purge(files.clone()),
                )
            }
        };
        let mut ok = false;
        let mut cancel = false;
        let modal = egui::Modal::new(Id::new("confirm_dialog")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.strong(title);
            ui.add_space(4.0);
            ui.add(Label::new(body).wrap());
            ui.label(RichText::new(note).small().color(ui.visuals().weak_text_color()));
            ui.separator();
            ui.horizontal(|ui| {
                let red = ui.visuals().error_fg_color;
                if ui.button(RichText::new(danger).color(red)).on_hover_text("回车").clicked() {
                    ok = true;
                }
                if ui.button("取消").on_hover_text("Esc").clicked() {
                    cancel = true;
                }
            });
        });
        let enter = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
        if ok || enter {
            acts.push(act);
            self.confirm = None;
        } else if cancel || modal.should_close() {
            self.confirm = None;
        }
    }

    /// 原文没保存就要离开：保存（回车）/ 不保存 / 继续编辑（Esc、点框外）
    fn leave_raw_dialog(&mut self, ctx: &egui::Context, next: Option<Sel>) {
        let Some(raw) = &self.raw else {
            self.confirm = None;
            return;
        };
        let name = file_label(&raw.path);
        // 0 保存，1 不保存，2 继续编辑
        let mut choice: Option<u8> = None;
        let modal = egui::Modal::new(Id::new("leave_raw")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.strong(format!("「{name}」改了还没保存"));
            ui.add_space(4.0);
            ui.label("离开前要保存吗？");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("保存").on_hover_text("回车").clicked() {
                    choice = Some(0);
                }
                let red = ui.visuals().error_fg_color;
                if ui.button(RichText::new("不保存").color(red)).clicked() {
                    choice = Some(1);
                }
                if ui.button("继续编辑").on_hover_text("Esc").clicked() {
                    choice = Some(2);
                }
            });
        });
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
            choice = Some(0);
        }
        if choice.is_none() && modal.should_close() {
            choice = Some(2);
        }
        let Some(choice) = choice else { return };
        self.confirm = None;
        // 中间栏已经画完了，再画一帧显示切换后的结果
        ctx.request_repaint();
        if choice == 2 {
            return;
        }
        if choice == 0 {
            self.save_raw();
            if self.store.error.is_some() {
                // 没保存成功，留在编辑区
                return;
            }
        }
        self.raw = None;
        if let Some(sel) = next {
            self.sel = sel;
        }
    }

    /// 中间栏的回收站：勾选批量彻底删除、清空；每项可以还原或彻底删除
    fn trash_view(&mut self, ui: &mut Ui, acts: &mut Vec<Act>) {
        ui.horizontal(|ui| {
            ui.strong("回收站");
            ui.label(
                RichText::new("删除的文档在这里，可以还原；彻底删除后无法恢复")
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
        });
        ui.separator();
        let all: Vec<String> = self.store.trash.iter().map(|e| e.file.clone()).collect();
        self.trash_checked.retain(|f| all.contains(f));
        if all.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new("回收站是空的").color(ui.visuals().weak_text_color()));
            return;
        }

        // 批量操作
        ui.horizontal(|ui| {
            let mut all_on = self.trash_checked.len() == all.len();
            if clear_checkbox(ui, &mut all_on, "全选").changed() {
                if all_on {
                    self.trash_checked = all.iter().cloned().collect();
                } else {
                    self.trash_checked.clear();
                }
            }
            let n = self.trash_checked.len();
            let red = ui.visuals().error_fg_color;
            if ui
                .add_enabled(n > 0, egui::Button::new(RichText::new(format!("彻底删除选中（{n}）")).color(red)))
                .clicked()
            {
                self.confirm = Some(Confirm::Purge(self.trash_checked.iter().cloned().collect()));
            }
            if ui.button(RichText::new("清空回收站").color(red)).clicked() {
                self.confirm = Some(Confirm::Purge(all.clone()));
            }
        });
        ui.add_space(4.0);

        let prefs = uip(ui);
        ScrollArea::vertical().id_salt("trash_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = prefs.gap;
            for e in self.store.trash.clone() {
                let fill = block_fill(ui.visuals(), false, false);
                egui::Frame::new()
                    .fill(fill)
                    .corner_radius(prefs.radius)
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let mut on = self.trash_checked.contains(&e.file);
                            if clear_checkbox(ui, &mut on, "").changed() {
                                if on {
                                    self.trash_checked.insert(e.file.clone());
                                } else {
                                    self.trash_checked.remove(&e.file);
                                }
                            }
                            // 先放右边的按钮，剩下的宽度给名字（过长截断）
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let red = ui.visuals().error_fg_color;
                                if ui.button(RichText::new("彻底删除").color(red)).clicked() {
                                    self.confirm = Some(Confirm::Purge(vec![e.file.clone()]));
                                }
                                if ui.button("还原").on_hover_text(format!("还原到 {}", e.original)).clicked() {
                                    acts.push(Act::Restore(e.file.clone()));
                                }
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    let name = Path::new(&e.original)
                                        .file_name()
                                        .map(|n| n.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    ui.add(Label::new(format!("📄 {name}")).truncate());
                                    ui.add(
                                        Label::new(
                                            RichText::new(format!(
                                                "原位置：{} · {} 条 · {}",
                                                e.original,
                                                e.items,
                                                time_ago(e.deleted_at)
                                            ))
                                            .small()
                                            .color(ui.visuals().weak_text_color()),
                                        )
                                        .truncate(),
                                    );
                                });
                            });
                        });
                    });
            }
        });
    }

    /// 外观设置窗口：改了立刻生效
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let before = self.store.prefs.ui.clone();
        let mut open = true;
        let mut make_guide = false;
        egui::Window::new("UI设置")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(ctx.screen_rect().center() - egui::vec2(170.0, 140.0))
            .show(ctx, |ui| {
                // 设置窗口自己的文字固定用默认字号：否则拖字号时窗口里的标签跟着变宽变窄，
                // 拖动条被推着左右挪，数值来回跳，整个界面跟着抖
                ui.style_mut().text_styles = egui::Style::default().text_styles;
                let p = &mut self.store.prefs.ui;
                egui::Grid::new("settings_grid").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                    ui.label("主题");
                    ui.horizontal_wrapped(|ui| {
                        ui.set_max_width(320.0);
                        ui.radio_value(&mut p.theme, 0, "跟随系统");
                        ui.radio_value(&mut p.theme, 1, "浅色");
                        ui.radio_value(&mut p.theme, 2, "深色");
                        ui.radio_value(&mut p.theme, 3, "护眼");
                        ui.radio_value(&mut p.theme, 5, "樱米");
                        ui.radio_value(&mut p.theme, 6, "薰衣草");
                        ui.radio_value(&mut p.theme, 7, "深海");
                        ui.radio_value(&mut p.theme, 8, "薄荷");
                        ui.radio_value(&mut p.theme, 9, "羊皮纸");
                    });
                    ui.end_row();

                    ui.label("强调色");
                    ui.horizontal(|ui| {
                        for (i, (name, _)) in ACCENTS.iter().enumerate() {
                            let side = ui.spacing().interact_size.y + 2.0;
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), Sense::click());
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, *name));
                            let on = p.accent as usize == i;
                            ui.painter().rect_filled(rect.shrink(2.0), side / 2.0, accent_swatch(i));
                            if on || resp.hovered() {
                                let c = ui.visuals().strong_text_color();
                                let w = if on { 2.0_f32 } else { 1.0_f32 };
                                ui.painter().circle_stroke(rect.center(), side / 2.0 - 0.5, Stroke::new(w, c));
                            }
                            if resp.on_hover_text(*name).clicked() {
                                p.accent = i as u8;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("字号");
                    stepper(ui, &mut p.font_size, 10.0, 22.0, 0.5, "");
                    ui.end_row();

                    ui.label("小字大小").on_hover_text("标签、文件名、提示等小字，是正文字号的百分之几");
                    stepper(ui, &mut p.small_pct, 60.0, 100.0, 5.0, "%");
                    ui.end_row();

                    ui.label("卡片间距");
                    stepper(ui, &mut p.gap, 0.0, 16.0, 1.0, "");
                    ui.end_row();

                    ui.label("圆角");
                    stepper(ui, &mut p.radius, 0.0, 12.0, 1.0, "");
                    ui.end_row();

                    ui.label("左栏块高度");
                    stepper(ui, &mut p.nav_rows, 1.0, 3.0, 0.25, " 行");
                    ui.end_row();

                    ui.label("光标闪烁");
                    clear_checkbox(ui, &mut p.cursor_blink, "闪烁")
                        .on_hover_text("关闭后光标常亮。闪烁时程序在前台会一直重画，约占一个核 5%");
                    ui.end_row();
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("恢复默认").clicked() {
                        *p = UiPrefs::default();
                    }
                    if ui
                        .button("重新生成使用说明")
                        .on_hover_text("在数据目录里生成一份「使用说明.md」（已有的不会被覆盖）")
                        .clicked()
                    {
                        make_guide = true;
                    }
                    ui.label(
                        RichText::new("改了立刻生效，自动保存")
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            });
        self.settings_open = open;
        if make_guide {
            if let Some(p) = self.store.install_guide() {
                self.goto(Sel::File(p));
            }
            ctx.request_repaint();
        }
        if self.store.prefs.ui != before {
            apply_ui_prefs(ctx, &self.store.prefs.ui);
            self.ui_dirty = true;
            self.row_h = Default::default();
        }
    }
}

/// 界面测试：用 egui_kittest 在内存里跑界面、模拟鼠标，不需要打开窗口
#[cfg(test)]
mod ui_tests {
    use super::*;
    use egui::text::{CCursor, CCursorRange};
    use egui::text_edit::TextEditState;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    fn harness(name: &str, content: &str) -> Harness<'static, App> {
        let root = std::env::temp_dir().join(format!("sucard-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(INBOX), content).unwrap();
        Harness::builder()
            .with_size(egui::vec2(1100.0, 700.0))
            .build_state(|ctx, app: &mut App| app.ui(ctx), App::with_store(Store::open(root)))
    }

    fn pointer(h: &mut Harness<'static, App>, pos: egui::Pos2, button: egui::PointerButton, pressed: bool) {
        h.input_mut().events.push(egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.step();
    }

    fn right_click(h: &mut Harness<'static, App>, pos: egui::Pos2) {
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        h.step();
        pointer(h, pos, egui::PointerButton::Secondary, true);
        pointer(h, pos, egui::PointerButton::Secondary, false);
        h.run();
    }

    fn selection(h: &Harness<'static, App>, id: Id) -> Option<(usize, usize)> {
        let r = TextEditState::load(&h.ctx, id)?.cursor.char_range()?;
        let (a, b) = (r.primary.index, r.secondary.index);
        Some((a.min(b), a.max(b)))
    }

    /// 编辑菜单里的「复制」（排除「忽略复制」）
    fn copy_item<'a>(h: &'a Harness<'static, App>) -> egui_kittest::kittest::Node<'a> {
        h.get_all_by_label_contains("复制")
            .find(|n| n.label().is_some_and(|l| !l.contains("忽略") && !l.contains("选中")))
            .expect("找不到「复制」")
    }

    /// 进入编辑、选中前 2 个字，返回编辑框 id
    fn edit_and_select(h: &mut Harness<'static, App>, id: &str) -> Id {
        h.state_mut().apply(vec![Act::StartEdit(id.into(), 0)]);
        h.run();
        let edit_id = Id::new(("edit", id, 0u8));
        assert!(h.ctx.memory(|m| m.has_focus(edit_id)), "进入编辑后编辑框没有焦点");
        let mut st = TextEditState::load(&h.ctx, edit_id).unwrap();
        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(2))));
        st.store(&h.ctx, edit_id);
        h.run();
        edit_id
    }

    #[test]
    fn right_click_keeps_selection() {
        let mut h = harness("rclick", "- [ ] 第一行内容 ^aaa111\n");
        let edit_id = edit_and_select(&mut h, "aaa111");
        let rect = h.ctx.read_response(edit_id).unwrap().rect;
        right_click(&mut h, rect.right_center() - egui::vec2(10.0, 0.0));
        assert_eq!(selection(&h, edit_id), Some((0, 2)), "右键后选区丢了");
        assert!(h.state().editing.is_some());
    }

    #[test]
    fn copy_from_menu_keeps_editing() {
        let mut h = harness("copy", "- [ ] 第一行内容 ^aaa111\n");
        let edit_id = edit_and_select(&mut h, "aaa111");
        let rect = h.ctx.read_response(edit_id).unwrap().rect;
        right_click(&mut h, rect.center());
        copy_item(&h).simulate_click();
        h.run();
        assert!(h.state().editing.is_some(), "点「复制」后退出了编辑");
        assert!(h.ctx.memory(|m| m.has_focus(edit_id)), "点「复制」后编辑框没有拿回焦点");
        assert_eq!(selection(&h, edit_id), Some((0, 2)), "点「复制」后选区丢了");
    }

    #[test]
    fn cut_from_menu_removes_text_and_keeps_editing() {
        let mut h = harness("cut", "- [ ] 第一行内容 ^aaa111
");
        let edit_id = edit_and_select(&mut h, "aaa111");
        let rect = h.ctx.read_response(edit_id).unwrap().rect;
        right_click(&mut h, rect.center());
        h.get_by_label_contains("剪切").simulate_click();
        h.run();
        let editing = h.state().editing.clone();
        assert_eq!(editing.map(|e| e.1).as_deref(), Some("行内容"), "剪切后文字不对或退出了编辑");
        assert!(h.ctx.memory(|m| m.has_focus(edit_id)));
        // 剪切还没保存：文件不变，回车后才写入
        h.press_key(Key::Enter);
        h.run();
        assert!(h.state().editing.is_none());
        assert_eq!(h.state().store.items["aaa111"].text, "行内容");
    }

    /// 在某条卡片的正文上点右键（中间栏那一张）
    fn right_click_card(h: &mut Harness<'static, App>, text: &str) {
        let r = h.get_all_by_label(text).next().expect("找不到卡片").raw_bounds().expect("卡片没有位置");
        let pos = egui::pos2(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32);
        right_click(h, pos);
    }

    #[test]
    fn card_menu_edits_tags() {
        let mut h = harness("tags", "- [ ] 线路 #日常 ^aaa111
  第二行
- [ ] 其他 #灵感 ^bbb222
");
        right_click_card(&mut h, "线路 …");
        h.get_by_label("编辑标签…").simulate_click();
        h.run();
        assert!(h.query_by_label("删除").is_none(), "标签面板里不该同时出现「删除」");
        // 加上「灵感」、去掉「日常」，菜单一直开着
        h.get_by_label("# 灵感").simulate_click();
        h.run();
        h.get_by_label("# 日常").simulate_click();
        h.run();
        assert_eq!(h.state().store.items["aaa111"].text, "线路 #灵感
第二行");
        assert!(h.query_by_label("清除全部标签").is_some(), "改完一个标签菜单就关了");
        // 清除全部
        h.get_by_label("清除全部标签").simulate_click();
        h.run();
        assert_eq!(h.state().store.items["aaa111"].text, "线路
第二行");
        assert!(h.query_by_label("清除全部标签").is_none());
        assert!(h.state().editing.is_none());
        // 关掉再打开，从主菜单开始
        h.press_key(Key::Escape);
        h.run();
        right_click_card(&mut h, "线路 …");
        assert!(h.query_by_label("编辑标签…").is_some() && h.query_by_label("← 返回").is_none());
    }

    #[test]
    fn tag_panel_marks_parent_with_child() {
        let mut h = harness("parent", "- [ ] 线路 #资料/AI ^aaa111
- [ ] 其他 #灵感 ^bbb222
");
        right_click_card(&mut h, "线路");
        h.get_by_label("编辑标签…").simulate_click();
        h.run();
        assert!(h.query_by_label("# 资料 · 已有子标签").is_some(), "父标签没标出「已有子标签」");
        assert!(h.query_by_label("# 灵感").is_some(), "没有子标签的不该标");
    }

    #[test]
    fn right_todo_and_done_views() {
        let mut h = harness("rightdone", "- [ ] 一 #资料 ^aaa111\n- [x] 二 #资料 ^bbb222\n");
        h.run();
        assert!(h.query_by_label("TODO：").is_some());
        assert_eq!(h.query_all_by_label("按标签").count(), 1, "只有左栏有「按标签」，右栏不该再有");
        // 手动排序：只有没完成的（中间栏 + 右栏各一张）；已完成的只在中间栏
        assert_eq!(h.query_all_by_label("一").count(), 2);
        assert_eq!(h.query_all_by_label("二").count(), 1);

        // 已完成：只有勾掉的
        h.get_by_label("已完成").simulate_click();
        h.run();
        assert!(h.state().store.prefs.right_done);
        assert!(state_json(&h).contains("\"right_done\": true"));
        assert_eq!(h.query_all_by_label("一").count(), 1);
        assert_eq!(h.query_all_by_label("二").count(), 2);

        // 「隐藏已完成」只管中间栏，右栏的「已完成」照样显示
        h.state_mut().store.prefs.hide_done = true;
        h.run();
        assert_eq!(h.query_all_by_label("二").count(), 1);

        // 在「已完成」里取消勾选：回到待办
        h.state_mut().apply(vec![Act::SetDone("bbb222".into(), false)]);
        h.run();
        assert!(h.query_by_label("还没有已完成的条目").is_some());
    }

    #[test]
    fn right_done_view_cannot_drag() {
        let order = |h: &Harness<'static, App>| h.state().store.order.clone();
        for show_done in [true, false] {
            let content = if show_done {
                "- [x] 甲 ^aaa111\n- [x] 乙 ^bbb222\n"
            } else {
                "- [ ] 甲 ^aaa111\n- [ ] 乙 ^bbb222\n"
            };
            let mut h = harness(if show_done { "nodrag1" } else { "nodrag2" }, content);
            h.state_mut().store.prefs.right_done = show_done;
            h.run();
            let before = order(&h);
            // 右栏的卡片（x 更大的那张）
            let card = |h: &Harness<'static, App>, t: &str| {
                h.get_all_by_label(t).filter_map(|n| n.raw_bounds()).max_by(|a, b| a.x0.total_cmp(&b.x0)).unwrap()
            };
            let from = card(&h, "乙");
            let to = card(&h, "甲");
            let p0 = egui::pos2(from.x0 as f32 + 5.0, ((from.y0 + from.y1) / 2.0) as f32);
            let p1 = egui::pos2(to.x0 as f32 + 5.0, to.y0 as f32 + 2.0);
            h.input_mut().events.push(egui::Event::PointerMoved(p0));
            h.step();
            pointer(&mut h, p0, egui::PointerButton::Primary, true);
            for k in 1..=6 {
                let p = p0 + (p1 - p0) * (k as f32 / 6.0);
                h.input_mut().events.push(egui::Event::PointerMoved(p));
                h.step();
            }
            pointer(&mut h, p1, egui::PointerButton::Primary, false);
            h.run();
            if show_done {
                assert_eq!(order(&h), before, "「已完成」里不该能拖动排序");
            } else {
                assert_ne!(order(&h), before, "手动排序时应该能拖动排序");
            }
        }
    }

    /// 搜索框旁的 ×（两个 × 都固定显示，搜索的在第二行，取靠下的那个）
    fn clear_btn(h: &Harness<'static, App>) -> Option<egui::Rect> {
        let r = h
            .query_all_by_role_and_label(egui::accesskit::Role::Button, "×")
            .filter_map(|n| n.raw_bounds())
            .max_by(|a, b| a.y0.total_cmp(&b.y0))?;
        Some(egui::Rect::from_min_max(
            egui::pos2(r.x0 as f32, r.y0 as f32),
            egui::pos2(r.x1 as f32, r.y1 as f32),
        ))
    }

    /// 在某个位置双击左键
    fn double_click(h: &mut Harness<'static, App>, pos: egui::Pos2) {
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        h.step();
        // 测试每帧前进 0.25 秒，双击要在 0.3 秒内：每次单击（按下 + 松开）放在同一帧
        for _ in 0..2 {
            for pressed in [true, false] {
                h.input_mut().events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            h.step();
        }
        h.run();
    }

    /// 在某个位置点一下左键
    fn click_at(h: &mut Harness<'static, App>, pos: egui::Pos2) {
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        h.step();
        pointer(h, pos, egui::PointerButton::Primary, true);
        pointer(h, pos, egui::PointerButton::Primary, false);
        h.run();
    }

    #[test]
    fn search_clear_button() {
        let mut h = harness("search", "- [ ] 线路 ^aaa111
- [ ] 其他 ^bbb222
");
        assert!(clear_btn(&h).is_some(), "清空按钮应固定显示");
        h.state_mut().search = "线路".into();
        h.run();
        assert!(h.query_by_label("其他").is_none(), "搜索没生效");
        // 点在按钮的角上（不是 × 字本身），整块都应该有效
        let r = clear_btn(&h).unwrap();
        click_at(&mut h, r.min + egui::vec2(2.0, 2.0));
        assert_eq!(h.state().search, "");
        assert!(clear_btn(&h).is_some(), "清空后按钮仍应显示");
        assert!(h.query_all_by_label("其他").next().is_some(), "清空后应显示全部");
        assert!(h.ctx.memory(|m| m.has_focus(Id::new("search"))), "清空后光标应留在搜索框");
    }

    #[test]
    fn quick_add_clear_button() {
        let mut h = harness("quickclear", "");
        h.state_mut().quick = "写到一半 #灵感".into();
        h.run();
        // 两个 × 都固定显示，快速添加的在上面一行
        let r = h
            .query_all_by_role_and_label(egui::accesskit::Role::Button, "×")
            .filter_map(|n| n.raw_bounds())
            .min_by(|a, b| a.y0.total_cmp(&b.y0))
            .map(|b| egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)))
            .unwrap();
        // 按钮在快速添加输入框的左边
        // 用界面结构树里的位置（read_response 可能是上一帧的，那时按钮还没出现）
        let input_left = h
            .query_all_by_role(egui::accesskit::Role::TextInput)
            .filter_map(|n| n.raw_bounds())
            .find(|b| (b.y0 as f32 - r.top()).abs() < 1.0)
            .expect("找不到快速添加输入框")
            .x0 as f32;
        assert!(r.right() <= input_left, "清空按钮应在输入框左边: btn={r:?} input_left={input_left}");
        click_at(&mut h, r.max - egui::vec2(2.0, 2.0));
        assert_eq!(h.state().quick, "");
        assert!(h.ctx.memory(|m| m.has_focus(Id::new("quick_add"))), "清空后光标应留在输入框");
        assert!(h.state().store.items.is_empty(), "清空不该保存任何内容");
    }

    #[test]
    fn quick_add_tag_panel() {
        let mut h = harness("quicktags", "- [ ] 一 #资料/AI ^aaa111
- [ ] 二 #灵感 ^bbb222
");
        h.state_mut().quick = "写脚本 #资料/AI".into();
        h.run();
        h.get_by_label("# 标签 1 ▼").simulate_click();
        h.run();
        // 和卡片的「编辑标签…」一样：父标签提示、有清除全部
        assert!(h.query_by_label("# 资料 · 已有子标签").is_some());
        assert!(h.query_by_label("清除全部标签").is_some());
        // 点「灵感」加上，菜单不关
        h.get_by_label("# 灵感").simulate_click();
        h.run();
        assert_eq!(h.state().quick, "写脚本 #资料/AI #灵感 ");
        assert!(h.query_by_label("清除全部标签").is_some(), "点一个标签菜单就关了");
        // 在面板的输入框里新建标签（面板里那个输入框宽 180 左右）
        let input = h
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .find(|n| n.raw_bounds().is_some_and(|b| (180.0..200.0).contains(&(b.x1 - b.x0))))
            .expect("找不到新建标签输入框");
        input.type_text("新 标签");
        h.run();
        h.press_key(Key::Enter);
        h.run();
        assert_eq!(h.state().quick, "写脚本 #资料/AI #灵感 #新标签 ");
        // 清除全部：只去掉标签，文字留着；还没保存任何东西
        h.get_by_label("清除全部标签").simulate_click();
        h.run();
        assert_eq!(h.state().quick, "写脚本");
        assert_eq!(h.state().store.items.len(), 2);
    }

    #[test]
    fn raw_editor_right_click_keeps_selection_and_menu_works() {
        let content = "# 标题
- [ ] 第一行内容 ^aaa111
";
        let mut h = harness("raw", content);
        h.state_mut().raw = Some(RawEdit::new(PathBuf::from(INBOX), content.to_string()));
        h.run();
        let id = Id::new("raw_edit");
        h.ctx.memory_mut(|m| m.request_focus(id));
        let mut st = TextEditState::load(&h.ctx, id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(2), CCursor::new(4))));
        st.store(&h.ctx, id);
        h.run();
        let rect = h.ctx.read_response(id).unwrap().rect;
        right_click(&mut h, rect.left_top() + egui::vec2(60.0, 8.0));
        assert_eq!(selection(&h, id), Some((2, 4)), "右键后选区丢了");
        assert!(h.query_by_label("返回（不保存）").is_some(), "编辑原文没有右键菜单");
        copy_item(&h).simulate_click();
        h.run();
        assert!(h.state().raw.is_some(), "点「复制」后退出了编辑原文");
        assert_eq!(selection(&h, id), Some((2, 4)), "点「复制」后选区丢了");
        assert!(h.ctx.memory(|m| m.has_focus(id)));
        // 菜单里的「返回（不保存）」：退出且文件不变
        right_click(&mut h, rect.left_top() + egui::vec2(60.0, 8.0));
        h.get_by_label("返回（不保存）").simulate_click();
        h.run();
        assert!(h.state().raw.is_none());
        assert_eq!(std::fs::read_to_string(h.state().store.root.join(INBOX)).unwrap(), content);
    }

    #[test]
    fn card_menu_copy_content() {
        let mut h = harness(
            "copycontent",
            "- [ ] 《歌名》 #歌词 #资料/AI ^aaa111
  男：第一句
  #只有标签的行
  女：第二句 #副歌
",
        );
        right_click_card(&mut h, "《歌名》 …");
        h.get_by_label("复制全部").simulate_click();
        // 复制命令只出现在点击那一帧的输出里，逐帧收集
        let mut copied = Vec::new();
        for _ in 0..6 {
            h.step();
            for c in &h.output().platform_output.commands {
                if let egui::OutputCommand::CopyText(t) = c {
                    copied.push(t.clone());
                }
            }
        }
        assert_eq!(copied, vec!["《歌名》
男：第一句
女：第二句".to_string()]);
        assert!(h.query_by_label("复制全部").is_none(), "复制后菜单应关闭");

        // %%备注%% 不复制；卡片上照样显示（灰色）
        let mut h = harness("copycomment", "- [ ] 正文 %%出处：某网站%% ^aaa111\n  %%整行备注%%\n  第二行\n");
        assert!(h.query_all_by_label("正文 %%出处：某网站%% …").count() >= 1, "卡片上应显示备注");
        right_click_card(&mut h, "正文 %%出处：某网站%% …");
        h.get_by_label("复制全部").simulate_click();
        let mut copied = Vec::new();
        for _ in 0..6 {
            h.step();
            for c in &h.output().platform_output.commands {
                if let egui::OutputCommand::CopyText(t) = c {
                    copied.push(t.clone());
                }
            }
        }
        assert_eq!(copied, vec!["正文\n第二行".to_string()]);
        assert!(h.query_by_label("复制全部").is_none(), "复制后菜单应关闭");
    }

    fn state_json(h: &Harness<'static, App>) -> String {
        std::fs::read_to_string(h.state().store.root.join(".sucard/state.json")).unwrap_or_default()
    }

    #[test]
    fn settings_window_changes_and_saves() {
        let mut h = harness("settings", "- [ ] 线路 ^aaa111\n");
        let default_sel = h.ctx.style_of(egui::Theme::Light).visuals.selection.bg_fill;
        h.get_by_label("UI设置").simulate_click();
        h.run();
        assert!(h.query_by_label("恢复默认").is_some(), "设置窗口没打开");

        // 重新生成使用说明：生成文档并选中它
        h.get_by_label("重新生成使用说明").simulate_click();
        h.run();
        assert!(h.state().store.root.join("使用说明.md").is_file());
        assert!(h.state().sel == Sel::File(PathBuf::from("使用说明.md")));
        assert!(h.state().store.items.len() > 20);

        // 换强调色：立刻生效，并写进 state.json
        h.get_by_label("绿").simulate_click();
        h.run();
        assert_eq!(h.state().store.prefs.ui.accent, 1);
        assert_ne!(h.ctx.style_of(egui::Theme::Light).visuals.selection.bg_fill, default_sel);
        assert!(state_json(&h).contains("\"accent\": 1"), "设置没保存: {}", state_json(&h));

        // 切深色
        h.get_by_label("深色").simulate_click();
        h.run();
        assert_eq!(h.ctx.theme(), egui::Theme::Dark);

        // 恢复默认
        h.get_by_label("恢复默认").simulate_click();
        h.run();
        assert_eq!(h.state().store.prefs.ui, UiPrefs::default());
        assert_eq!(h.ctx.style_of(egui::Theme::Light).visuals.selection.bg_fill, default_sel);
        assert!(state_json(&h).contains("\"accent\": 0"));
    }

    /// 左栏某个块里文字的上边缘位置。上面的「全部」块越高，下面块里的文字就越靠下，
    /// 用它来判断块高度设置有没有生效
    fn first_nav_label_y(h: &Harness<'static, App>, label: &str) -> f64 {
        h.get_all_by_label(label).next().unwrap().raw_bounds().unwrap().y0
    }

    #[test]
    fn font_size_and_nav_height_take_effect() {
        let mut h = harness("prefs", "- [ ] 线路 ^aaa111\n");
        let body = |h: &Harness<'static, App>| {
            h.ctx.style_of(egui::Theme::Light).text_styles[&egui::TextStyle::Body].size
        };
        assert_eq!(body(&h), 12.5);
        let small = |h: &Harness<'static, App>| {
            h.ctx.style_of(egui::Theme::Light).text_styles[&egui::TextStyle::Small].size
        };
        assert!((small(&h) - 12.5 * 0.85).abs() < 0.01, "小号字应是正文的 85%: {}", small(&h));
        // 左栏：「全部」下面紧跟着第一个文件块；块越高，文件名越往下
        let y2 = first_nav_label_y(&h, "📥 收件箱");
        h.state_mut().store.prefs.ui.nav_rows = 3.0;
        h.state_mut().store.prefs.ui.font_size = 16.0;
        // 直接改设置时要手动应用（设置窗口里改会自动应用）
        apply_ui_prefs(&h.ctx.clone(), &h.state().store.prefs.ui.clone());
        h.run();
        assert_eq!(body(&h), 16.0);
        assert!((small(&h) - 16.0 * 0.85).abs() < 0.01, "小号字没跟着缩放: {}", small(&h));
        let y3 = first_nav_label_y(&h, "📥 收件箱");
        assert!(y3 > y2 + 10.0, "左栏块没有变高: {y2} -> {y3}");
    }

    #[test]
    fn font_size_slider_drag_is_stable() {
        let mut h = harness("fontdrag", "- [ ] 线路 ^aaa111\n");
        h.get_by_label("UI设置").simulate_click();
        h.run();
        // 第一个拖动条是字号
        let r = h
            .get_all_by_role(egui::accesskit::Role::Slider)
            .filter_map(|n| n.raw_bounds())
            .min_by(|a, b| a.y0.total_cmp(&b.y0))
            .expect("找不到字号拖动条");
        let y = ((r.y0 + r.y1) / 2.0) as f32;
        let mut x = (r.x0 + 10.0) as f32;
        h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(x, y)));
        h.step();
        pointer(&mut h, egui::pos2(x, y), egui::PointerButton::Primary, true);
        // 往右慢慢拖：字号只能变大，不能来回跳
        let mut last = h.state().store.prefs.ui.font_size;
        for _ in 0..12 {
            x += 6.0;
            h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(x, y)));
            h.step();
            let now = h.state().store.prefs.ui.font_size;
            assert!(now >= last, "往右拖字号反而变小了: {last} -> {now}");
            last = now;
        }
        assert!(last > 12.5, "拖动没有改变字号");
        // 鼠标停住不动：字号不能再变（之前会来回跳，界面跟着抖）
        let mut seen = vec![];
        for _ in 0..10 {
            h.step();
            seen.push(h.state().store.prefs.ui.font_size);
        }
        assert!(seen.iter().all(|v| *v == last), "鼠标停住后字号还在变: {last} -> {seen:?}");
        pointer(&mut h, egui::pos2(x, y), egui::PointerButton::Primary, false);
        h.run();
        assert_eq!(h.state().store.prefs.ui.font_size, last);
    }

    /// UI设置 窗口里第 row 行（字号 0、小字大小 1、卡片间距 2、圆角 3、左栏块高度 4）的「-」或「+」按钮
    fn step_btn(h: &Harness<'static, App>, label: &str, row: usize) -> egui::Pos2 {
        let mut all: Vec<_> = h
            .get_all_by_role_and_label(egui::accesskit::Role::Button, label)
            .filter_map(|n| n.raw_bounds())
            .collect();
        all.sort_by(|a, b| a.y0.total_cmp(&b.y0));
        let b = all[row];
        egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
    }

    #[test]
    fn settings_steppers_and_cursor_blink() {
        let mut h = harness("stepper", "- [ ] 线路 ^aaa111\n");
        // 光标闪烁默认关闭
        assert!(!h.ctx.style_of(egui::Theme::Light).visuals.text_cursor.blink);
        h.get_by_label("UI设置").simulate_click();
        h.run();

        // 字号 +、-：每次 0.5
        let plus = step_btn(&h, "+", 0);
        click_at(&mut h, plus);
        assert_eq!(h.state().store.prefs.ui.font_size, 13.0);
        let minus = step_btn(&h, "-", 0);
        click_at(&mut h, minus);
        click_at(&mut h, minus);
        assert_eq!(h.state().store.prefs.ui.font_size, 12.0);
        // 小字大小 -：每次 5%，并且立刻生效
        let minus = step_btn(&h, "-", 1);
        click_at(&mut h, minus);
        assert_eq!(h.state().store.prefs.ui.small_pct, 80.0);
        let small = h.ctx.style_of(egui::Theme::Light).text_styles[&egui::TextStyle::Small].size;
        assert!((small - 12.0 * 0.80).abs() < 0.01, "小字大小没生效: {small}");
        // 左栏块高度 +：每次 0.25
        let plus = step_btn(&h, "+", 4);
        click_at(&mut h, plus);
        assert_eq!(h.state().store.prefs.ui.nav_rows, 2.25);
        // 到上限后「+」不再生效
        h.state_mut().store.prefs.ui.radius = 12.0;
        h.run();
        let plus = step_btn(&h, "+", 3);
        click_at(&mut h, plus);
        assert_eq!(h.state().store.prefs.ui.radius, 12.0);

        // 打开光标闪烁
        h.get_by_label("闪烁").simulate_click();
        h.run();
        assert!(h.state().store.prefs.ui.cursor_blink);
        assert!(h.ctx.style_of(egui::Theme::Light).visuals.text_cursor.blink);
        assert!(state_json(&h).contains("\"cursor_blink\": true"));
    }

    #[test]
    fn single_line_inputs_are_taller_and_match_clear_button() {
        let h = harness("inputs", "");
        let text_h = h.ctx.style_of(egui::Theme::Light).text_styles[&egui::TextStyle::Body].size;
        let inputs: Vec<_> = h
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .filter_map(|n| n.raw_bounds())
            .collect();
        assert_eq!(inputs.len(), 3, "应有快速添加、搜索、新建文档 3 个输入框");
        for b in &inputs {
            let hgt = (b.y1 - b.y0) as f32;
            assert!(hgt >= text_h + 2.0 * INPUT_PAD_Y as f32, "输入框不够高: {hgt}");
        }
        // 快速添加旁边的 × 和输入框一样高
        let quick = inputs.iter().min_by(|a, b| a.y0.total_cmp(&b.y0)).unwrap();
        let x = h
            .get_all_by_role_and_label(egui::accesskit::Role::Button, "×")
            .filter_map(|n| n.raw_bounds())
            .min_by(|a, b| a.y0.total_cmp(&b.y0))
            .unwrap();
        assert!(((x.y1 - x.y0) - (quick.y1 - quick.y0)).abs() < 1.5, "× 和输入框不一样高: {x:?} {quick:?}");
    }

    #[test]
    fn top_bar_small_texts_follow_font_size() {
        let mut h = harness("smalltext", "");
        let root = h.state().store.root.display().to_string();
        let height = |h: &Harness<'static, App>, label: &str| {
            let b = h.get_by_label(label).raw_bounds().unwrap();
            (b.y1 - b.y0) as f32
        };
        let target0 = height(&h, "→ 收件箱.md");
        let root0 = height(&h, &root);
        h.state_mut().store.prefs.ui.font_size = 20.0;
        apply_ui_prefs(&h.ctx.clone(), &h.state().store.prefs.ui.clone());
        h.run();
        let target1 = height(&h, "→ 收件箱.md");
        let root1 = height(&h, &root);
        assert!(target1 > target0 * 1.3, "「→ 收件箱.md」没跟着变大: {target0} -> {target1}");
        assert!(root1 > root0 * 1.3, "目录路径没跟着变大: {root0} -> {root1}");
    }

    #[test]
    fn delete_asks_for_confirmation() {
        let mut h = harness("confirmdel", "- [ ] 线路 ^aaa111\n- [ ] 其他 ^bbb222\n");
        let file = |h: &Harness<'static, App>| std::fs::read_to_string(h.state().store.root.join(INBOX)).unwrap();

        // 右键「删除」：弹确认框，取消后什么都不变
        right_click_card(&mut h, "线路");
        h.get_by_label("删除").simulate_click();
        h.run();
        assert!(h.query_by_label("删除这一条？").is_some(), "没弹确认框");
        assert!(h.state().store.items.contains_key("aaa111"));
        h.get_by_label("取消").simulate_click();
        h.run();
        assert!(h.query_by_label("删除这一条？").is_none());
        assert!(h.state().store.items.contains_key("aaa111"));
        assert!(file(&h).contains("^aaa111"));

        // 再来一次，点「删除」才真正删除
        right_click_card(&mut h, "线路");
        h.get_by_label("删除").simulate_click();
        h.run();
        h.get_by_role_and_label(egui::accesskit::Role::Button, "删除").simulate_click();
        h.run();
        assert!(!h.state().store.items.contains_key("aaa111"));
        assert!(!file(&h).contains("^aaa111"));

        // 键盘：选中后按 Delete 弹框，Esc 取消；再按 Delete、回车 删除
        h.state_mut().selected = Some(("bbb222".into(), 0));
        h.press_key(Key::Delete);
        h.run();
        assert!(h.query_by_label("删除这一条？").is_some());
        h.press_key(Key::Escape);
        h.run();
        assert!(h.query_by_label("删除这一条？").is_none());
        assert!(h.state().store.items.contains_key("bbb222"), "Esc 不该删除");
        assert!(h.state().editing.is_none(), "确认框里的按键不该触发卡片快捷键");
        h.press_key(Key::Delete);
        h.run();
        h.press_key(Key::Enter);
        h.run();
        assert!(!h.state().store.items.contains_key("bbb222"));
        assert!(h.state().editing.is_none(), "回车确认删除，不该同时进入编辑");
    }

    #[test]
    fn emptying_text_asks_before_delete() {
        let mut h = harness("emptydel", "- [ ] 线路 ^aaa111
");
        h.state_mut().apply(vec![Act::StartEdit("aaa111".into(), 0)]);
        h.run();
        h.state_mut().editing.as_mut().unwrap().1.clear();
        h.press_key(Key::Enter);
        h.run();
        assert!(h.query_by_label("删除这一条？").is_some(), "内容删光后保存应先确认");
        assert!(h.state().editing.is_none());
        // 取消：这条保留原来的内容
        h.get_by_label("取消").simulate_click();
        h.run();
        assert_eq!(h.state().store.items["aaa111"].text, "线路");
    }

    /// 确认框里的确认按钮（和页面上同名按钮区分：确认框在最上层，排在最后）
    fn confirm_btn(h: &Harness<'static, App>, label: &str) -> egui::Pos2 {
        let b = h
            .get_all_by_role_and_label(egui::accesskit::Role::Button, label)
            .filter_map(|n| n.raw_bounds())
            .last()
            .unwrap();
        egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32)
    }

    #[test]
    fn delete_doc_to_trash_restore_and_purge() {
        let mut h = harness("trashui", "- [ ] 收件箱里的 ^aaa111\n");
        std::fs::write(h.state().store.root.join("b.md"), "- [ ] 第一条 ^bbb111\n- [ ] 第二条 ^bbb222\n").unwrap();
        std::fs::write(h.state().store.root.join("c.md"), "- [ ] 丙 ^ccc111\n").unwrap();
        h.state_mut().store.reload();
        h.run();

        // 左栏文件块右键 → 删除文档… → 确认框 → 取消：什么都不变
        right_click_card(&mut h, "📄 b.md");
        h.get_by_label("删除文档…").simulate_click();
        h.run();
        assert!(h.query_by_label("删除文档「b.md」？").is_some(), "没弹确认框");
        assert!(h.query_by_label("其中 2 条素材会一起移到回收站。").is_some());
        h.get_by_label("取消").simulate_click();
        h.run();
        assert!(h.state().store.root.join("b.md").exists());

        // 再来一次，确认：文件进回收站，条目消失
        right_click_card(&mut h, "📄 b.md");
        h.get_by_label("删除文档…").simulate_click();
        h.run();
        h.press_key(Key::Enter);
        h.run();
        assert!(!h.state().store.root.join("b.md").exists());
        assert!(!h.state().store.items.contains_key("bbb111"));
        assert_eq!(h.state().store.trash.len(), 1);
        // c.md 也删掉，方便测批量
        right_click_card(&mut h, "📄 c.md");
        h.get_by_label("删除文档…").simulate_click();
        h.run();
        h.press_key(Key::Enter);
        h.run();
        assert_eq!(h.state().store.trash.len(), 2);

        // 打开回收站，还原 c.md
        h.get_by_label("🗑 回收站").simulate_click();
        h.run();
        assert!(h.query_by_label("📄 b.md").is_some() && h.query_by_label("📄 c.md").is_some());
        let restore: Vec<_> = h
            .get_all_by_role_and_label(egui::accesskit::Role::Button, "还原")
            .filter_map(|n| n.raw_bounds())
            .collect();
        assert_eq!(restore.len(), 2);
        // 最近删除的在最上面：第一个「还原」是 c.md
        let b = restore.iter().min_by(|a, b| a.y0.total_cmp(&b.y0)).unwrap();
        click_at(&mut h, egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32));
        assert!(h.state().store.root.join("c.md").exists(), "没还原");
        assert!(h.state().store.items.contains_key("ccc111"));
        assert_eq!(h.state().store.trash.len(), 1);

        // 批量：全选 → 彻底删除选中（1）→ 确认
        h.get_by_label("全选").simulate_click();
        h.run();
        h.get_by_label("彻底删除选中（1）").simulate_click();
        h.run();
        assert!(h.query_by_label("彻底删除 1 个文档？").is_some());
        assert!(h.query_by_label("彻底删除后无法恢复。").is_some());
        let ok = confirm_btn(&h, "彻底删除");
        click_at(&mut h, ok);
        assert!(h.state().store.trash.is_empty());
        assert!(h.query_by_label("回收站是空的").is_some());
        let left = std::fs::read_dir(h.state().store.root.join(".sucard/回收站")).unwrap().count();
        assert_eq!(left, 1, "回收站目录里应只剩 index.json");
    }

    /// 容量实测（平时不跑）：cargo test --release capacity -- --ignored --nocapture
    #[test]
    #[ignore]
    fn capacity_benchmark() {
        for n in [1_000usize, 5_000, 20_000] {
            let mut content = String::new();
            for i in 0..n {
                content.push_str(&format!("- [ ] 第 {i} 条素材，写一点正文内容做测试 #标签{} ^id{i:06}
", i % 20));
            }
            let mut h = harness(&format!("cap{n}"), &content);
            h.run();
            let t = std::time::Instant::now();
            let frames = 20;
            for _ in 0..frames {
                h.ctx.request_repaint();
                h.step();
            }
            let frame_ms = t.elapsed().as_secs_f64() * 1000.0 / frames as f64;
            let t = std::time::Instant::now();
            h.state_mut().store.set_done("id000000", true);
            let save_ms = t.elapsed().as_secs_f64() * 1000.0;
            let size_kb = content.len() as f64 / 1024.0;
            eprintln!("BENCH {n:>6} 条  文件 {size_kb:>7.0} KB  每帧 {frame_ms:>7.1} ms  改一条保存 {save_ms:>6.1} ms");
        }
    }

    #[test]
    fn virtual_scroll_draws_only_visible_cards() {
        let mut content = String::new();
        for i in 0..3000 {
            content.push_str(&format!("- [ ] 条目{i:04} #t{} ^id{i:05}\n", i % 3));
        }
        let mut h = harness("virtual", &content);
        h.run();
        // 中间栏和右栏都只画了视口附近的几十张，不是 3000 张
        let drawn = h.query_all_by_label_contains("条目").count();
        assert!(drawn > 4 && drawn < 200, "画了 {drawn} 张");
        assert!(h.query_all_by_label("条目0000").count() >= 1);
        assert_eq!(h.query_all_by_label("条目2999").count(), 0);

        // 把中间栏滚到底：最后一条出现
        for _ in 0..40 {
            h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(500.0, 300.0)));
            h.input_mut().events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Page,
                delta: egui::vec2(0.0, -50.0),
                modifiers: Default::default(),
            });
            // 滚动后提示框有延时重绘，用固定步数
            h.run_steps(3);
        }
        assert!(h.query_all_by_label("条目2999").count() >= 1, "滚到底看不到最后一条");
        // 右栏没滚，第一条还在；中间栏滚走了，只剩一处
        assert_eq!(h.query_all_by_label("条目0000").count(), 1);
    }

    #[test]
    fn fixed_docs_below_trash_and_not_deletable() {
        let mut h = harness("fixedui", "- [ ] 收件箱里的 ^aaa111\n");
        std::fs::write(h.state().store.root.join("b.md"), "- [ ] 乙 ^bbb111\n").unwrap();
        h.state_mut().store.reload();
        h.run();
        let top = |h: &Harness<'static, App>, label: &str| h.get_by_label(label).raw_bounds().unwrap().y0;
        // 顺序：普通文档 → 回收站 → 收件箱 → 保管箱；普通列表里不再重复出现
        assert!(top(&h, "📄 b.md") < top(&h, "🗑 回收站"));
        assert!(top(&h, "🗑 回收站") < top(&h, "📥 收件箱"));
        assert!(top(&h, "📥 收件箱") < top(&h, "🔒 保管箱"));
        assert!(h.query_by_label("📄 收件箱.md").is_none());

        // 点保管箱：选中它；没有「修改标题」
        h.get_by_label("🔒 保管箱").simulate_click();
        h.run();
        assert!(h.state().sel == Sel::File(PathBuf::from("保管箱.md")));
        assert!(h.query_by_label("修改标题").is_none());
        assert!(h.query_by_label("编辑原文").is_some());

        // 右键：「删除文档…」是灰的，点了也没反应
        right_click_card(&mut h, "📥 收件箱");
        h.get_by_label("删除文档…").simulate_click();
        h.run();
        assert!(h.state().confirm.is_none());
        assert!(h.state().store.root.join(INBOX).is_file());
    }

    #[test]
    fn raw_editor_dirty_save_and_leave() {
        let content = "# 标题\n- [ ] 第一 #资料 ^aaa111\n";
        let mut h = harness("rawdirty", content);
        let disk = |h: &Harness<'static, App>| std::fs::read_to_string(h.state().store.root.join(INBOX)).unwrap();
        h.state_mut().sel = Sel::File(PathBuf::from(INBOX));
        h.run();
        h.get_by_label("编辑原文").simulate_click();
        h.run();
        assert!(h.query_by_label_contains("2 行 ·").is_some() && h.query_by_label_contains("· 1 条素材").is_some(), "状态栏不对");
        assert!(h.query_by_label("● 未保存").is_none());

        // 改一下：显示未保存，状态栏跟着变
        h.state_mut().raw.as_mut().unwrap().buf.push_str("- [ ] 新的\n");
        h.state_mut().raw.as_mut().unwrap().recount();
        h.run();
        assert!(h.query_by_label("● 未保存").is_some());
        assert!(h.query_by_label_contains("3 行 ·").is_some() && h.query_by_label_contains("· 2 条素材").is_some());

        // 点左栏「全部」：先问；继续编辑 → 什么都不变
        h.get_by_label("全部").simulate_click();
        h.run();
        assert!(h.query_by_label("离开前要保存吗？").is_some(), "没有提示保存");
        h.get_by_label("继续编辑").simulate_click();
        h.run();
        assert!(h.state().raw.is_some() && h.state().sel == Sel::File(PathBuf::from(INBOX)));

        // Ctrl+S：保存并留在编辑区；新条目补上了 ^id
        h.input_mut().events.push(egui::Event::Key {
            key: Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
        h.run();
        assert!(h.state().raw.is_some(), "Ctrl+S 后不该退出");
        assert!(h.query_by_label("● 未保存").is_none());
        assert!(disk(&h).contains("- [ ] 新的 ^"), "保存后没写盘或没补 id: {}", disk(&h));
        assert_eq!(h.state().raw.as_ref().unwrap().buf, disk(&h));

        // 没改动时 Esc 直接返回
        h.press_key(Key::Escape);
        h.run();
        assert!(h.state().raw.is_none());

        // 改了按 Esc → 问 → 不保存：返回，文件不变
        h.get_by_label("编辑原文").simulate_click();
        h.run();
        let before = disk(&h);
        h.state_mut().raw.as_mut().unwrap().buf.push_str("随便\n");
        h.run();
        h.press_key(Key::Escape);
        h.run();
        h.get_by_label("不保存").simulate_click();
        h.run();
        assert!(h.state().raw.is_none());
        assert_eq!(disk(&h), before);

        // 改了点「全部」→ 问 → 回车 = 保存并切过去
        h.get_by_label("编辑原文").simulate_click();
        h.run();
        h.state_mut().raw.as_mut().unwrap().buf.push_str("- [ ] 再来 ^ccc333\n");
        h.run();
        h.get_by_label("全部").simulate_click();
        h.run();
        h.press_key(Key::Enter);
        h.run();
        assert!(h.state().raw.is_none() && h.state().sel == Sel::All);
        assert!(disk(&h).contains("再来 ^ccc333"));
    }

    #[test]
    fn markdown_highlight_keeps_text_and_colors_parts() {
        let st = HiStyle {
            size_bits: 12.5_f32.to_bits(),
            dark: false,
            text: Color32::BLACK,
            weak: Color32::GRAY,
            accent: Color32::RED,
            link: Color32::BLUE,
            mark: Color32::YELLOW,
        };
        let text = "# 标题\n- [ ] 内容 #资料/AI ^abc123\n- [x] 完成了\n    续行\n普通 #灵感\r\n";
        let job = highlight_markdown(&st, text);
        assert_eq!(job.text, text, "着色不能改动文字");
        let color_of = |needle: &str| {
            let at = job.text.find(needle).unwrap();
            job.sections.iter().find(|s| s.byte_range.contains(&at)).unwrap().format.clone()
        };
        assert_eq!(color_of("- [ ]").color, Color32::RED);
        assert_ne!(color_of("- [ ]").background, Color32::TRANSPARENT, "勾选框没底色");
        assert_eq!(color_of("内容").background, Color32::TRANSPARENT);
        assert_eq!(color_of("#资料").color, tag_colors("资料/AI", false).1);
        assert_ne!(color_of("^abc").color, Color32::BLACK);
        assert_eq!(color_of("标题").background, Color32::TRANSPARENT, "标题不再着色");
        assert_eq!(color_of("完成了").color, Color32::GRAY);
        assert!(color_of("完成了").strikethrough.width > 0.0);
        assert!(color_of("续行").strikethrough.width > 0.0, "已完成条目的续行也要变灰");
        assert_eq!(color_of("普通").color, Color32::BLACK);
        assert_eq!(color_of("#灵感").color, tag_colors("灵感", false).1);

        // %%备注%%：可跨行，盖掉标签色等其他着色
        let text = "- [ ] 正文 %%备注 #标签\n  第二行%% 后面 ^abc123\n";
        let job = highlight_markdown(&st, text);
        assert_eq!(job.text, text);
        let fmt_at = |needle: &str| {
            let at = job.text.find(needle).unwrap();
            job.sections.iter().find(|s| s.byte_range.contains(&at)).unwrap().format.clone()
        };
        assert_eq!(fmt_at("正文").color, Color32::BLACK);
        assert_eq!(fmt_at("%%备注").color, Color32::GRAY);
        assert_eq!(fmt_at("#标签").color, Color32::GRAY);
        assert_eq!(fmt_at("第二行").color, Color32::GRAY);
        assert_eq!(fmt_at("后面").color, Color32::BLACK);
        // 网址：链接色 + 下划线；备注里的网址保持灰色但有下划线
        let text2 = "看 https://a.com/x 和 %%www.b.com%%\n";
        let job2 = highlight_markdown(&st, text2);
        let f2 = |needle: &str| {
            let at = job2.text.find(needle).unwrap();
            job2.sections.iter().find(|s| s.byte_range.contains(&at)).unwrap().format.clone()
        };
        assert_eq!(f2("https").color, Color32::BLUE);
        assert!(f2("https").underline.width > 0.0);
        assert_eq!(f2("看").underline.width, 0.0);
        assert_eq!(f2("www").color, Color32::GRAY);
        let job3 = highlight_markdown(&st, "前 ==重点== 后\n");
        let bg = |needle: &str| {
            let at = job3.text.find(needle).unwrap();
            job3.sections.iter().find(|s| s.byte_range.contains(&at)).unwrap().format.background
        };
        assert_eq!(bg("==重点"), Color32::YELLOW);
        assert_eq!(bg("后"), Color32::TRANSPARENT);
        assert!(f2("www").underline.width > 0.0);
        // 段落要首尾相接
        let mut pos = 0;
        for s in &job.sections {
            assert_eq!(s.byte_range.start, pos);
            pos = s.byte_range.end;
        }
        assert_eq!(pos, text.len());
    }

    #[test]
    fn ignore_copy_button_and_shortcut() {
        let content = "- [ ] 正文 出处 ^aaa111\n";
        let mut h = harness("ignorecopy", content);
        h.state_mut().sel = Sel::File(PathBuf::from(INBOX));
        h.state_mut().raw = Some(RawEdit::new(PathBuf::from(INBOX), content.to_string()));
        h.run();
        let id = Id::new("raw_edit");
        let select = |h: &mut Harness<'static, App>, a: usize, b: usize| {
            let mut st = TextEditState::load(&h.ctx, id).unwrap_or_default();
            st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
            st.store(&h.ctx, id);
            h.run();
        };
        let buf = |h: &Harness<'static, App>| h.state().raw.as_ref().unwrap().buf.clone();

        // 选中「出处」（第 9、10 个字）点按钮：加上 %%，选区跟着包住整段
        select(&mut h, 9, 11);
        h.get_by_label("忽略复制").simulate_click();
        h.run();
        assert_eq!(buf(&h), "- [ ] 正文 %%出处%% ^aaa111\n");
        assert_eq!(selection(&h, id), Some((9, 15)));
        assert!(h.query_by_label("● 未保存").is_some());

        // 再点一次：去掉
        h.get_by_label("忽略复制").simulate_click();
        h.run();
        assert_eq!(buf(&h), content);

        // 只选中记号里面的文字也能取消
        select(&mut h, 9, 11);
        h.get_by_label("忽略复制").simulate_click();
        h.run();
        select(&mut h, 11, 13);
        h.get_by_label("忽略复制").simulate_click();
        h.run();
        assert_eq!(buf(&h), content);

        // Ctrl+/ 快捷键，Ctrl+Z 能撤销
        h.ctx.memory_mut(|m| m.request_focus(id));
        select(&mut h, 6, 8);
        h.input_mut().events.push(egui::Event::Key {
            key: Key::Slash,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
        h.run();
        assert_eq!(buf(&h), "- [ ] %%正文%% 出处 ^aaa111\n");
        h.input_mut().events.push(egui::Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
        h.run();
        assert_eq!(buf(&h), content, "Ctrl+Z 没撤销");
    }

    /// 逐帧收集「打开网址」和「复制」命令
    fn collect_cmds(h: &mut Harness<'static, App>) -> (Vec<String>, Vec<String>) {
        let (mut opened, mut copied) = (Vec::new(), Vec::new());
        for _ in 0..6 {
            h.step();
            for c in &h.output().platform_output.commands {
                match c {
                    egui::OutputCommand::OpenUrl(u) => opened.push(u.url.clone()),
                    egui::OutputCommand::CopyText(t) => copied.push(t.clone()),
                    _ => {}
                }
            }
        }
        (opened, copied)
    }

    fn click_mod(h: &mut Harness<'static, App>, pos: egui::Pos2, modifiers: egui::Modifiers) {
        h.input_mut().modifiers = modifiers;
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        h.step();
        for pressed in [true, false] {
            h.input_mut().events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            });
            // 松开那一帧留给 collect_cmds 跑（命令只出现在那一帧的输出里）
            if pressed {
                h.step();
            }
        }
    }

    #[test]
    fn card_links_ctrl_click_and_menu() {
        let mut h = harness("links", "- [ ] https://example.com/abcdefghijklmn 看看 ^aaa111\n");
        h.run();
        let label = "https://example.com/abcdefghijklmn 看看";
        let b = h.get_all_by_label(label).next().unwrap().raw_bounds().unwrap();
        let on_url = egui::pos2(b.x0 as f32 + 30.0, ((b.y0 + b.y1) / 2.0) as f32);

        // 普通单击：选中，不打开
        click_mod(&mut h, on_url, egui::Modifiers::NONE);
        let (opened, _) = collect_cmds(&mut h);
        assert!(opened.is_empty(), "单击不该打开: {opened:?}");
        assert!(h.state().selected.is_some());

        // Ctrl+单击网址：打开
        click_mod(&mut h, on_url, egui::Modifiers::COMMAND);
        let (opened, _) = collect_cmds(&mut h);
        assert_eq!(opened, vec!["https://example.com/abcdefghijklmn".to_string()]);

        // Ctrl+单击网址以外的文字：不打开
        let off_url = egui::pos2(b.x1 as f32 - 6.0, ((b.y0 + b.y1) / 2.0) as f32);
        click_mod(&mut h, off_url, egui::Modifiers::COMMAND);
        let (opened, _) = collect_cmds(&mut h);
        assert!(opened.is_empty(), "点到「看看」不该打开: {opened:?}");
        h.input_mut().modifiers = egui::Modifiers::NONE;

        // 右键：打开链接 / 复制链接
        right_click_card(&mut h, label);
        h.get_by_label("复制链接").simulate_click();
        let (_, copied) = collect_cmds(&mut h);
        assert_eq!(copied, vec!["https://example.com/abcdefghijklmn".to_string()]);
        right_click_card(&mut h, label);
        h.get_by_label("打开链接").simulate_click();
        let (opened, _) = collect_cmds(&mut h);
        assert_eq!(opened.len(), 1);

        // 多个网址（含备注里的）：逐个列出 + 复制全部；www. 打开时补 https://
        let mut h = harness("links2", "- [ ] 两个 www.a.com 和 %%出处 http://b.cn/x%% ^aaa111\n");
        right_click_card(&mut h, "两个 www.a.com 和 %%出处 http://b.cn/x%%");
        assert!(h.query_by_label("打开 b.cn/x").is_some());
        h.get_by_label("打开 www.a.com").simulate_click();
        let (opened, _) = collect_cmds(&mut h);
        assert_eq!(opened, vec!["https://www.a.com".to_string()]);
        right_click_card(&mut h, "两个 www.a.com 和 %%出处 http://b.cn/x%%");
        h.get_by_label("复制全部链接（2）").simulate_click();
        let (_, copied) = collect_cmds(&mut h);
        assert_eq!(copied, vec!["www.a.com\nhttp://b.cn/x".to_string()]);
    }

    #[test]
    fn raw_editor_ctrl_click_opens_link() {
        let content = "https://example.com/abcdefghijklmnop\n- [ ] 其他 ^aaa111\n";
        let mut h = harness("rawlink", content);
        h.state_mut().raw = Some(RawEdit::new(PathBuf::from(INBOX), content.to_string()));
        h.run();
        let rect = h.ctx.read_response(Id::new("raw_edit")).unwrap().rect;
        let pos = rect.left_top() + egui::vec2(40.0, 14.0);
        click_mod(&mut h, pos, egui::Modifiers::NONE);
        let (opened, _) = collect_cmds(&mut h);
        assert!(opened.is_empty(), "普通单击不该打开");
        click_mod(&mut h, pos, egui::Modifiers::COMMAND);
        let (opened, _) = collect_cmds(&mut h);
        assert_eq!(opened, vec!["https://example.com/abcdefghijklmnop".to_string()]);
    }

    #[test]
    fn partial_copy_marked_text() {
        let content = "- [ ] 标题 ==重点== 其他 ^aaa111\n  ==第二行== 普通\n- [ ] 没有标记 ^bbb222\n";
        let mut h = harness("partcopy", content);
        right_click_card(&mut h, "标题 ==重点== 其他 …");
        h.get_by_label("部分复制").simulate_click();
        let (_, copied) = collect_cmds(&mut h);
        assert_eq!(copied, vec!["重点\n第二行".to_string()]);
        // 复制全部：还是全部正文
        right_click_card(&mut h, "标题 ==重点== 其他 …");
        h.get_by_label("复制全部").simulate_click();
        let (_, copied) = collect_cmds(&mut h);
        assert_eq!(copied, vec!["标题 ==重点== 其他\n==第二行== 普通".to_string()]);
        // 没有标记：「部分复制」是灰的，点了不复制
        right_click_card(&mut h, "没有标记");
        h.get_by_label("部分复制").simulate_click();
        let (_, copied) = collect_cmds(&mut h);
        assert!(copied.is_empty());

        // 编辑原文：选中后点「复制选中」加上 ==，Ctrl+E 再取消
        h.press_key(Key::Escape);
        h.run();
        h.state_mut().raw = Some(RawEdit::new(PathBuf::from(INBOX), "- [ ] 甲乙丙 ^ccc333\n".into()));
        h.run();
        let id = Id::new("raw_edit");
        let mut st = TextEditState::load(&h.ctx, id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(6), CCursor::new(8))));
        st.store(&h.ctx, id);
        h.run();
        h.get_by_label("复制选中").simulate_click();
        h.run();
        assert_eq!(h.state().raw.as_ref().unwrap().buf, "- [ ] ==甲乙==丙 ^ccc333\n");
        h.ctx.memory_mut(|m| m.request_focus(id));
        h.run();
        h.input_mut().events.push(egui::Event::Key {
            key: Key::E,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
        h.run();
        assert_eq!(h.state().raw.as_ref().unwrap().buf, "- [ ] 甲乙丙 ^ccc333\n");
    }

    #[test]
    fn read_view_renders_and_switches() {
        // 排版：标题去掉 #，勾选框换成 □ / ■，^id 不显示，网址带下划线
        let st = HiStyle {
            size_bits: 12.5_f32.to_bits(),
            dark: false,
            text: Color32::BLACK,
            weak: Color32::GRAY,
            accent: Color32::RED,
            link: Color32::BLUE,
            mark: Color32::YELLOW,
        };
        let content = "# 大标题\n- [ ] 第一条 #资料 ^aaa111\n  看 https://example.com/abc 吧 ==重点==\n- [x] 做完了 ^bbb222\n";
        let job = render_reading(&st, content);
        assert_eq!(job.text, "大标题\n□ 第一条 #资料\n  看 https://example.com/abc 吧 ==重点==\n■ 做完了\n");
        let fmt_at = |needle: &str| {
            let at = job.text.find(needle).unwrap();
            job.sections.iter().find(|s| s.byte_range.contains(&at)).unwrap().format.clone()
        };
        assert!(fmt_at("大标题").font_id.size > 12.5);
        assert_eq!(fmt_at("□").color, Color32::RED);
        assert_eq!(fmt_at("#资料").color, tag_colors("资料", false).1);
        assert!(fmt_at("https").underline.width > 0.0);
        assert_eq!(fmt_at("==重点").background, Color32::YELLOW);
        assert!(fmt_at("做完了").strikethrough.width > 0.0);

        // 界面：阅读原文 → 编辑原文 → 返回；Esc 退出阅读
        let mut h = harness("reading", content);
        h.state_mut().sel = Sel::File(PathBuf::from(INBOX));
        h.run();
        h.get_by_label("阅读原文").simulate_click();
        h.run();
        assert!(h.query_by_label("阅读原文：收件箱.md").is_some());
        assert!(h.query_by_label_contains("□ 第一条").is_some(), "没显示排版后的内容");

        // 文件在外面被改了：自动重新读
        std::fs::write(h.state().store.root.join(INBOX), "- [ ] 外面改的 ^ccc333\n").unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        let f = std::fs::File::options().write(true).open(h.state().store.root.join(INBOX)).unwrap();
        f.set_modified(later).unwrap();
        drop(f);
        h.run();
        assert!(h.query_by_label_contains("□ 外面改的").is_some(), "文件改了没重新读");

        h.get_by_label("编辑原文").simulate_click();
        h.run();
        assert!(h.state().raw.is_some() && h.state().reading.is_none());
        h.press_key(Key::Escape);
        h.run();
        assert!(h.state().raw.is_none());

        h.get_by_label("阅读原文").simulate_click();
        h.run();
        h.press_key(Key::Escape);
        h.run();
        assert!(h.state().reading.is_none());
        assert!(h.query_by_label("阅读原文").is_some(), "返回后应回到卡片视图");
    }

    #[test]
    fn drop_files_imports_and_hints() {
        let mut h = harness("dropui", "- [ ] 原有 ^aaa111\n");
        let outside = std::env::temp_dir().join(format!("sucard-drop-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("外部.md"), "- [ ] 拖进来的 ^zzz999\n").unwrap();
        std::fs::write(outside.join("图.png"), [0u8]).unwrap();

        // 拖动中：显示提示（画在最上层，不影响布局）
        h.input_mut().hovered_files.push(egui::HoveredFile { path: Some(outside.join("外部.md")), ..Default::default() });
        h.step();
        assert!(h.state().store.error.is_none());

        // 松开：.md 导入并选中；.png 提示没导入
        h.input_mut().hovered_files.clear();
        for f in ["外部.md", "图.png"] {
            h.input_mut().dropped_files.push(egui::DroppedFile { path: Some(outside.join(f)), ..Default::default() });
        }
        h.run();
        let root = h.state().store.root.clone();
        assert!(root.join("外部.md").is_file());
        assert!(h.state().sel == Sel::File(PathBuf::from("外部.md")));
        assert!(h.state().store.items.contains_key("zzz999"));
        assert!(h.state().store.error.as_deref().is_some_and(|e| e.contains("图.png")));
        assert!(h.query_all_by_label("拖进来的").count() >= 1);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn tag_tree_folds_and_untagged_label() {
        let mut h = harness(
            "fold",
            "- [ ] 一 #资料/AI ^aaa111\n- [ ] 二 #资料/书 ^bbb222\n- [ ] 三 #灵感 ^ccc333\n- [ ] 四 ^ddd444\n",
        );
        h.state_mut().store.prefs.left_by_tag = true;
        h.run();
        assert!(h.query_by_label("无标签").is_some());
        assert!(h.query_by_label("未分类").is_none());
        assert!(h.query_by_label("# AI").is_some() && h.query_by_label("# 书").is_some());

        // 点「资料」前面的箭头：子标签收起，资料本身没被选中；设置记住
        let b = h.get_by_label("# 资料").raw_bounds().unwrap();
        let arrow = egui::pos2(b.x0 as f32 - FOLD_W / 2.0, ((b.y0 + b.y1) / 2.0) as f32);
        click_mod(&mut h, arrow, egui::Modifiers::NONE);
        h.run();
        assert!(h.query_by_label("# AI").is_none() && h.query_by_label("# 书").is_none(), "没折叠");
        assert!(h.query_by_label("# 灵感").is_some());
        assert!(h.state().sel != Sel::Tag("资料".into()), "点箭头不该选中");
        assert_eq!(h.state().store.prefs.collapsed_tags, vec!["资料".to_string()]);
        assert!(state_json(&h).contains("资料"));

        // 再点展开
        click_mod(&mut h, arrow, egui::Modifiers::NONE);
        h.run();
        assert!(h.query_by_label("# AI").is_some());
        assert!(h.state().store.prefs.collapsed_tags.is_empty());

        // 双击块：折叠，再双击展开；双击没有子标签的「灵感」不会折叠
        let b = h.get_by_label("# 资料").raw_bounds().unwrap();
        let block = egui::pos2(b.x0 as f32 + 20.0, ((b.y0 + b.y1) / 2.0) as f32);
        double_click(&mut h, block);
        assert!(h.query_by_label("# AI").is_none(), "双击没折叠");
        double_click(&mut h, block);
        assert!(h.query_by_label("# AI").is_some(), "再双击没展开");

        // 没有子标签的「灵感」前面没有箭头：点那里就是选中
        let b = h.get_by_label("# 灵感").raw_bounds().unwrap();
        click_mod(&mut h, egui::pos2(b.x0 as f32 - FOLD_W / 2.0, ((b.y0 + b.y1) / 2.0) as f32), egui::Modifiers::NONE);
        h.run();
        assert!(h.state().sel == Sel::Tag("灵感".into()));
    }

    #[test]
    fn doc_not_in_todo() {
        let mut h = harness("notodoui", "- [ ] 收件箱的 ^aaa111\n");
        std::fs::write(h.state().store.root.join("b.md"), "- [ ] 资料一条 ^bbb111\n").unwrap();
        h.state_mut().store.reload();
        h.run();
        assert!(h.query_by_label("TODO：").is_some());
        // 全部视图下：中间栏 + 右栏各一张
        assert_eq!(h.query_all_by_label("资料一条").count(), 2);

        right_click_card(&mut h, "📄 b.md");
        h.get_by_label("不放入TODO").simulate_click();
        h.run();
        assert!(!h.state().store.in_todo(Path::new("b.md")));
        assert_eq!(h.query_all_by_label("资料一条").count(), 1, "右栏还在显示");
        assert_eq!(h.query_all_by_label("收件箱的").count(), 2, "别的文档不受影响");
        assert!(state_json(&h).contains("not_in_todo"));

        // 再勾掉：回到右栏
        right_click_card(&mut h, "📄 b.md");
        h.get_by_label("不放入TODO").simulate_click();
        h.run();
        assert_eq!(h.query_all_by_label("资料一条").count(), 2);
    }

    #[test]
    fn warm_themes() {
        let mut h = harness("eye", "- [ ] 一条 ^aaa111\n");
        h.get_by_label("UI设置").simulate_click();
        h.run();
        // (选项, 编号, 主色, 深色, 主题强调色)
        for (label, n, bg, dark, accent) in [
            ("护眼", 3u8, rgb(208, 195, 165), false, None),
            ("樱米", 5, rgb(0xF6, 0xE4, 0xC8), false, Some(rgb(0xE9, 0x52, 0x7D))),
            ("薰衣草", 6, rgb(0xDB, 0xD0, 0xE6), false, Some(rgb(0x4F, 0x4F, 0x85))),
            ("深海", 7, rgb(0x0E, 0x42, 0x58), true, Some(rgb(0x7E, 0x34, 0x27))),
            ("薄荷", 8, rgb(0xE0, 0xF0, 0xE9), false, Some(rgb(0x0D, 0x4D, 0x76))),
            ("羊皮纸", 9, rgb(0xE5, 0xD5, 0xB6), false, Some(rgb(0xB0, 0x5B, 0x24))),
        ] {
            h.get_by_label(label).simulate_click();
            h.run();
            assert_eq!(h.state().store.prefs.ui.theme, n);
            assert!(state_json(&h).contains(&format!("\"theme\": {n}")));
            let (pref, used, other) = if dark {
                (egui::ThemePreference::Dark, egui::Theme::Dark, egui::Theme::Light)
            } else {
                (egui::ThemePreference::Light, egui::Theme::Light, egui::Theme::Dark)
            };
            assert_eq!(h.ctx.options(|o| o.theme_preference), pref, "{label}");
            let v = h.ctx.style_of(used).visuals.clone();
            assert_eq!(v.panel_fill, bg, "{label} 主色不对");
            assert_eq!(v.dark_mode, dark);
            let c = warm(&v).expect("没认出配色主题");
            assert!(std::ptr::eq(c, warm_of_theme(n).unwrap()), "{label} 被认成了别的主题");
            assert_eq!(v.widgets.noninteractive.bg_stroke.color, c.line, "{label} 分隔线");
            assert_eq!(block_fill(&v, false, false), c.card[0], "{label} 卡片底色没换");
            assert!(warm(&h.ctx.style_of(other).visuals).is_none(), "{label} 另一套样式不该变");
            // 强调色是「默认蓝」时用主题自带的强调色
            if let Some(a) = accent {
                let expect = if dark { mix(a, Color32::BLACK, 0.45) } else { mix(a, Color32::WHITE, 0.55) };
                assert_eq!(v.selection.bg_fill, expect, "{label} 没用主题的强调色");
            }
        }
        // 选了别的强调色：以选的为准
        h.get_by_label("绿").simulate_click();
        h.run();
        let v = h.ctx.style_of(egui::Theme::Light).visuals.clone();
        assert_eq!(v.selection.bg_fill, mix(rgb(34, 160, 90), Color32::WHITE, 0.55));
        // 换回浅色：恢复默认配色
        h.get_by_label("浅色").simulate_click();
        h.run();
        assert!(warm(&h.ctx.style_of(egui::Theme::Light).visuals).is_none());
    }

    #[test]
    fn default_root_location() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| std::ffi::OsString::from(v))
        };
        // Windows：优先 USERPROFILE，即使设置了 HOME（比如从 Git 的窗口启动）
        let both = env(&[("USERPROFILE", r"C:\Users\amy"), ("HOME", r"D:\tools\home")]);
        assert_eq!(default_root(&both, true), PathBuf::from(r"C:\Users\amy").join("Sucard"));
        // 其他系统：用 HOME
        let unix = env(&[("HOME", "/home/amy"), ("USERPROFILE", "/x")]);
        assert_eq!(default_root(&unix, false), PathBuf::from("/home/amy").join("Sucard"));
        // 缺一个就用另一个；空值当作没有；都没有就用当前目录
        assert_eq!(default_root(env(&[("HOME", "/h")]), true), PathBuf::from("/h").join("Sucard"));
        assert_eq!(default_root(env(&[("USERPROFILE", ""), ("HOME", "/h")]), true), PathBuf::from("/h").join("Sucard"));
        assert_eq!(default_root(env(&[]), true), PathBuf::from(".").join("Sucard"));
    }

    #[test]
    fn edit_menu_has_no_tag_item() {
        let mut h = harness("notag", "- [ ] 第一行内容 #a ^aaa111
");
        let edit_id = edit_and_select(&mut h, "aaa111");
        let rect = h.ctx.read_response(edit_id).unwrap().rect;
        right_click(&mut h, rect.center());
        assert!(h.query_by_label_contains("忽略复制").is_some());
        copy_item(&h);
        assert!(h.query_by_label_contains("插入标签").is_none());
        assert!(h.query_by_label_contains("编辑标签").is_none());
    }

    #[test]
    fn click_outside_menu_saves_and_exits() {
        let mut h = harness("outside", "- [ ] 第一行内容 ^aaa111
");
        let edit_id = edit_and_select(&mut h, "aaa111");
        let rect = h.ctx.read_response(edit_id).unwrap().rect;
        right_click(&mut h, rect.center());
        assert!(h.query_by_label_contains("剪切").is_some(), "菜单没打开");
        // 在窗口底部空白处点一下
        let pos = egui::pos2(600.0, 680.0);
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        h.step();
        pointer(&mut h, pos, egui::PointerButton::Primary, true);
        pointer(&mut h, pos, egui::PointerButton::Primary, false);
        h.run();
        assert!(h.query_by_label_contains("剪切").is_none(), "菜单没关");
        assert!(h.state().editing.is_none(), "点外面后应保存并退出编辑");
    }
}
