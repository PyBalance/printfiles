use base64::engine::general_purpose::STANDARD as Base64;
use base64::Engine;
use chardetng::EncodingDetector;
use clap::{Parser, ValueEnum};
use globwalk::GlobWalkerBuilder;
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Reader {
    /// 直接按文本读取（默认）
    Text,
    /// 调用 macOS `textutil` 读取（适合 doc/docx/rtf/html 等）
    Textutil,
    /// 自动：对部分扩展名用 textutil，其它走 Text
    Auto,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BinaryStrategy {
    /// 跳过二进制文件
    Skip,
    /// 按十六进制输出
    Hex,
    /// 按 Base64 输出
    Base64,
    /// 强制按文本处理
    Print,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Divider {
    /// 形如 ===path=== / ===end of 'path'===
    Equals,
    /// 形如 ``` path/to/file
    TripleBacktick,
    /// 形如 <file path="path/to/file">
    XmlTag,
}

impl Divider {
    // 修改：增加 encoding 参数
    fn header(self, rel: &str, encoding: Option<&str>) -> String {
        match self {
            Divider::Equals => {
                let enc_info = encoding.map(|e| format!(" [{}]", e)).unwrap_or_default();
                format!("==={}{}===", rel, enc_info)
            }
            Divider::TripleBacktick => {
                let enc_info = encoding.map(|e| format!(" [{}]", e)).unwrap_or_default();
                format!("``` {}{}", rel, enc_info)
            }
            Divider::XmlTag => {
                let enc_attr = encoding
                    .map(|e| format!(" encoding=\"{}\"", e))
                    .unwrap_or_default();
                format!("<file path=\"{}\"{}>", escape_xml_attr(rel), enc_attr)
            }
        }
    }

    fn footer(self, rel: &str) -> String {
        match self {
            Divider::Equals => format!("===end of '{}'===", rel),
            Divider::TripleBacktick => "```".to_string(),
            Divider::XmlTag => "</file>".to_string(),
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "printfiles",
    version,
    about = "Print files matched by globs/dirs with ===header=== and ===end of 'file'==="
)]
struct Args {
    /// 一组以空格或逗号分隔的模式或目录
    #[arg(required = true)]
    items: Vec<String>,

    /// 读取后端：text(默认) / textutil / auto
    #[arg(long, value_enum, default_value_t = Reader::Text)]
    reader: Reader,

    /// 若传入目录，是否仅限这些扩展
    #[arg(long)]
    ext: Option<String>,

    /// 控制相对路径显示时的基目录
    #[arg(long)]
    relative_from: Option<PathBuf>,

    /// 最大文件大小（字节），超过则跳过
    #[arg(long)]
    max_size: Option<u64>,

    /// 当检测到可能是二进制文件时的处理策略
    #[arg(long, value_enum, default_value_t = BinaryStrategy::Skip)]
    binary: BinaryStrategy,

    /// 排序策略
    #[arg(long, value_enum, default_value_t = SortKey::Name)]
    sort: SortKey,

    /// 是否跟随符号链接
    #[arg(long, default_value_t = true)]
    follow_links: bool,

    /// 仅输出文件内容的前/后若干行
    #[arg(
        long,
        short = 'c',
        value_name = "N[:M]",
        num_args = 0..=1,
        default_missing_value = "5:3"
    )]
    clip: Option<String>,

    /// 输出分隔符风格
    #[arg(long, value_enum, default_value_t = Divider::Equals)]
    divider: Divider,

    /// 输出详细日志
    #[arg(long, action = clap::ArgAction::SetTrue)]
    verbose: bool,

    /// 安静模式
    #[arg(long, action = clap::ArgAction::SetTrue)]
    quiet: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SortKey {
    Name,
    Size,
    Mtime,
}

#[derive(Debug, Clone, Copy)]
struct ClipSpec {
    head: usize,
    tail: usize,
}

fn parse_clip_spec(raw: &str) -> anyhow::Result<ClipSpec> {
    let s = raw.trim();
    if s.is_empty() {
        return Ok(ClipSpec { head: 5, tail: 3 });
    }

    let (head_str, tail_str_opt) = match s.split_once(':') {
        Some((h, t)) => (h.trim(), Some(t.trim())),
        None => (s, None),
    };

    let head = if head_str.is_empty() {
        0
    } else {
        head_str.parse::<usize>().map_err(|e| {
            anyhow::anyhow!("invalid --clip value (head part '{}'): {}", head_str, e)
        })?
    };

    let tail = match tail_str_opt {
        Some(t) if !t.is_empty() => t.parse::<usize>().map_err(|e| {
            anyhow::anyhow!("invalid --clip value (tail part '{}'): {}", t, e)
        })?,
        _ => 0,
    };

    if head == 0 && tail == 0 {
        anyhow::bail!("invalid --clip value '{}': head and tail cannot both be 0", raw);
    }

    Ok(ClipSpec { head, tail })
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let logger = Logger::new(args.verbose, args.quiet);

    let clip_spec = match args.clip.as_deref() {
        Some(raw) => Some(parse_clip_spec(raw)?),
        None => None,
    };

    let relative_base = resolve_relative_base(args.relative_from.as_ref())?;

    let mut tokens: Vec<String> = Vec::new();
    for it in args.items.iter() {
        for piece in it.split(',') {
            let s = piece.trim();
            if !s.is_empty() {
                tokens.push(s.to_string());
            }
        }
    }

    if tokens.is_empty() {
        logger.warn("（未匹配到任何文件）");
        std::process::exit(2);
    }

    let mut files: BTreeSet<PathBuf> = BTreeSet::new();

    for token in tokens {
        let path = Path::new(&token);
        if path.is_dir() {
            if let Err(err) = collect_dir(path, args.ext.as_deref(), &mut files, args.follow_links)
            {
                logger.warn(&format!("目录遍历失败 {token}: {err}"));
            }
            continue;
        }

        match expand_glob(&token, args.follow_links) {
            Ok(paths) => {
                for path in paths {
                    if path.is_file() {
                        files.insert(normalize(&path));
                    }
                }
            }
            Err(err) => {
                logger.warn(&format!("模式无效或没有匹配: {err}"));
            }
        }
    }

    if files.is_empty() {
        logger.warn("（未匹配到任何文件）");
        std::process::exit(2);
    }

    let mut entries: Vec<FileEntry> = files
        .into_iter()
        .map(|path| {
            let len = file_len(&path).ok().flatten();
            let mtime = metadata_mtime(&path).ok().flatten();
            FileEntry { path, len, mtime }
        })
        .collect();

    sort_entries(&mut entries, args.sort);

    let mut out = io::BufWriter::new(io::stdout());
    let mut had_error = false;

    for entry in entries {
        let path = entry.path;
        let rel = rel_display(&path, relative_base.as_deref());
        logger.info(&format!("处理文件: {}", rel));

        // 逻辑修改：在这里处理文件大小限制
        // 如果超过限制，直接打印默认 Header 并跳过
        if let Some(limit) = args.max_size {
            if let Some(size) = entry.len {
                if size > limit {
                    logger.warn(&format!(
                        "提示: 跳过 {} (size={} > max_size={})",
                        path.display(),
                        size,
                        limit
                    ));
                    // 因为没有读取，不知道编码，传入 None
                    writeln!(out, "{}", args.divider.header(&rel, None))?;
                    writeln!(out, "(skipped: file exceeds max size)")?;
                    writeln!(out, "{}", args.divider.footer(&rel))?;
                    continue;
                }
            }
        }

        // 逻辑修改：将 divider 和 rel 传入 read_and_write，
        // 由内部函数在读取并探测编码后，负责打印 Header。
        match read_and_write(
            &path,
            &rel, // 新增参数
            args.divider, // 新增参数
            args.reader,
            args.binary,
            clip_spec,
            &logger,
            &mut out,
        ) {
            Ok(ended_with_newline) => {
                if !ended_with_newline {
                    writeln!(out)?;
                }
            }
            Err(err) => {
                logger.error(&format!("错误: 读取失败 {}: {err}", path.display()));
                had_error = true;
                // 只有在报错时（意味着内部可能没来得及打印 Header），
                // 这里不需要补 Header，因为 read_and_write 内部不同阶段报错的处理比较复杂。
                // 简单起见，如果 read_and_write 彻底失败，我们至少换行
                writeln!(out)?;
            }
        }

        let footer = args.divider.footer(&rel);
        writeln!(out, "{}", footer)?;
    }

    out.flush()?;
    if had_error {
        std::process::exit(1);
    }

    Ok(())
}

// ... (collect_dir, normalize, rel_display, strip_dot_slash 等辅助函数保持不变) ...
// 为了节省篇幅，这里省略了未修改的辅助函数，请保留原有的 ...

fn collect_dir(
    dir: &Path,
    exts: Option<&str>,
    files: &mut BTreeSet<PathBuf>,
    follow_links: bool,
) -> anyhow::Result<()> {
    let walker = GlobWalkerBuilder::from_patterns(dir, &["**/*"])
        .follow_links(follow_links)
        .case_insensitive(false)
        .build()?;
    for entry in walker.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() {
            if let Some(exts) = exts {
                if !ext_match(path, exts) {
                    continue;
                }
            }
            files.insert(normalize(path));
        }
    }
    Ok(())
}

fn normalize(p: &Path) -> PathBuf {
    PathBuf::from(p)
}

fn rel_display(p: &Path, base: Option<&Path>) -> String {
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(p)
    } else {
        p.to_path_buf()
    };

    if let Some(base) = base {
        if let Ok(stripped) = absolute.strip_prefix(base) {
            return strip_dot_slash(stripped).to_string();
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(stripped) = absolute.strip_prefix(&cwd) {
            return strip_dot_slash(stripped).to_string();
        }
    }

    strip_dot_slash(p).to_string()
}

fn strip_dot_slash(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix("./").unwrap_or(&s).to_string()
}

fn resolve_relative_base(from: Option<&PathBuf>) -> anyhow::Result<Option<PathBuf>> {
    let Some(base) = from else {
        return Ok(None);
    };
    if base.is_absolute() {
        return Ok(Some(base.clone()));
    }
    let cwd = std::env::current_dir()?;
    Ok(Some(cwd.join(base)))
}

fn file_len(path: &Path) -> anyhow::Result<Option<u64>> {
    match path.metadata() {
        Ok(meta) => Ok(Some(meta.len())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn metadata_mtime(path: &Path) -> anyhow::Result<Option<SystemTime>> {
    match path.metadata() {
        Ok(meta) => Ok(meta.modified().ok()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn sort_entries(entries: &mut [FileEntry], key: SortKey) {
    match key {
        SortKey::Name => entries.sort_by(|a, b| a.path.cmp(&b.path)),
        SortKey::Size => entries.sort_by(|a, b| {
            a.len
                .unwrap_or_default()
                .cmp(&b.len.unwrap_or_default())
                .then_with(|| a.path.cmp(&b.path))
        }),
        SortKey::Mtime => entries.sort_by(|a, b| {
            a.mtime
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .cmp(&b.mtime.unwrap_or(SystemTime::UNIX_EPOCH))
                .then_with(|| a.path.cmp(&b.path))
        }),
    }
}

fn ext_match(path: &Path, exts_csv: &str) -> bool {
    let ext = path
        .extension()
        .and_then(OsStr::to_str)
        .map(|s| s.to_ascii_lowercase());
    let Some(ext) = ext else {
        return false;
    };
    for e in exts_csv.split(',') {
        if ext == e.trim().to_ascii_lowercase() {
            return true;
        }
    }
    false
}

struct FileEntry {
    path: PathBuf,
    len: Option<u64>,
    mtime: Option<SystemTime>,
}

fn expand_glob(pattern: &str, follow_links: bool) -> anyhow::Result<Vec<PathBuf>> {
    let has_glob = pattern.contains('*') || pattern.contains('?') || pattern.contains('[');
    if !has_glob {
        return Ok(vec![PathBuf::from(pattern)]);
    }
    let walker = GlobWalkerBuilder::from_patterns(".", &[pattern])
        .follow_links(follow_links)
        .case_insensitive(false)
        .build()?;
    Ok(walker
        .filter_map(|e| e.ok())
        .map(|e| e.path().to_path_buf())
        .collect())
}

// 修改：返回 (解码内容, 编码名称)
// 如果是 UTF-8，编码名称为 None
fn decode_content(bytes: &[u8]) -> (Cow<'_, str>, Option<&'static str>) {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return (Cow::Borrowed(s), None);
    }

    let mut detector = EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    let (cow, _, _) = encoding.decode(bytes);
    // 返回检测到的编码名称（如 GBK, EUC-JP 等）
    (cow, Some(encoding.name()))
}

// 修改：read_and_write 签名增加 rel_path 和 divider
fn read_and_write<W: Write>(
    path: &Path,
    rel_path: &str,
    divider: Divider,
    reader: Reader,
    binary: BinaryStrategy,
    clip: Option<ClipSpec>,
    logger: &Logger,
    mut out: W,
) -> anyhow::Result<bool> {
    match reader {
        Reader::Text => write_text(path, rel_path, divider, binary, clip, logger, &mut out),
        Reader::Textutil => {
            write_textutil_then_fallback(path, rel_path, divider, binary, clip, logger, &mut out)
        }
        Reader::Auto => {
            if should_use_textutil(path) {
                write_textutil_then_fallback(
                    path, rel_path, divider, binary, clip, logger, &mut out,
                )
            } else {
                write_text(path, rel_path, divider, binary, clip, logger, &mut out)
            }
        }
    }
}

// 修改：write_text 负责打印 Header
fn write_text<W: Write>(
    path: &Path,
    rel_path: &str,
    divider: Divider,
    binary: BinaryStrategy,
    clip: Option<ClipSpec>,
    logger: &Logger,
    out: &mut W,
) -> anyhow::Result<bool> {
    match fs::read(path) {
        Ok(bytes) => {
            // 如果判定为二进制，先打印默认 Header（不带编码信息），再处理二进制内容
            if is_probably_binary(&bytes) && !matches!(binary, BinaryStrategy::Print) {
                writeln!(out, "{}", divider.header(rel_path, None))?;
                if handle_binary_content(path, &bytes, binary, logger, out)? {
                    return Ok(true);
                }
            }

            // 文本处理：先探测编码
            let (s, encoding_name) = decode_content(&bytes);

            // 打印带有编码信息的 Header
            writeln!(out, "{}", divider.header(rel_path, encoding_name))?;

            if let Some(clip) = clip {
                write_clipped(&s, clip, out)
            } else {
                write!(out, "{}", s)?;
                Ok(s.ends_with('\n'))
            }
        }
        Err(e) => {
            // 如果读取都失败了，打印一个默认 Header 然后抛出错误
            writeln!(out, "{}", divider.header(rel_path, None))?;
            anyhow::bail!("{}", e);
        }
    }
}

fn write_textutil_then_fallback<W: Write>(
    path: &Path,
    rel_path: &str,
    divider: Divider,
    binary: BinaryStrategy,
    clip: Option<ClipSpec>,
    logger: &Logger,
    out: &mut W,
) -> anyhow::Result<bool> {
    if which::which("textutil").is_ok() {
        let output = Command::new("textutil")
            .arg("-convert")
            .arg("txt")
            .arg("-stdout")
            .arg(path)
            .output();
        match output {
            Ok(outp) if outp.status.success() => {
                // textutil 转换后一定是 UTF-8，所以 Header 不显示特殊编码
                writeln!(out, "{}", divider.header(rel_path, None))?;
                
                if let Some(clip) = clip {
                    // 依然做一个 decode 以防万一
                    let (s, _) = decode_content(&outp.stdout);
                    return write_clipped(&s, clip, out);
                } else {
                    out.write_all(&outp.stdout)?;
                    return Ok(outp.stdout.ends_with(b"\n"));
                }
            }
            Ok(outp) => {
                logger.warn(&format!(
                    "警告: textutil 处理失败 ({}), 回退到文本读取: {}",
                    outp.status,
                    path.display()
                ));
            }
            Err(e) => {
                logger.warn(&format!(
                    "警告: textutil 调用异常 ({}), 回退到文本读取: {}",
                    e,
                    path.display()
                ));
            }
        }
    } else {
        logger.warn(&format!(
            "提示: 未检测到 textutil，回退到文本读取。文件: {}",
            path.display()
        ));
    }
    // 回退
    write_text(path, rel_path, divider, binary, clip, logger, out)
}

fn should_use_textutil(path: &Path) -> bool {
    let Some(ext) = path
        .extension()
        .and_then(OsStr::to_str)
        .map(|s| s.to_ascii_lowercase())
    else {
        return false;
    };
    matches!(
        ext.as_str(),
        "rtf" | "rtfd" | "doc" | "docx" | "html" | "htm" | "odt" | "webarchive"
    )
}

// 稍微重命名了一下，因为 handle_binary 现在只负责打印内容体
fn handle_binary_content<W: Write>(
    path: &Path,
    bytes: &[u8],
    strategy: BinaryStrategy,
    logger: &Logger,
    out: &mut W,
) -> anyhow::Result<bool> {
    match strategy {
        BinaryStrategy::Skip => {
            writeln!(out, "(skipped binary file)")?;
        }
        BinaryStrategy::Hex => {
            let encoded = hex::encode(bytes);
            writeln!(out, "{}", encoded)?;
        }
        BinaryStrategy::Base64 => {
            let encoded = Base64.encode(bytes);
            writeln!(out, "{}", encoded)?;
        }
        BinaryStrategy::Print => unreachable!(),
    }
    logger.warn(&format!(
        "提示: 二进制文件按 {:?} 处理: {}",
        strategy,
        path.display()
    ));
    Ok(true)
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

fn write_clipped<W: Write>(
    content: &str,
    clip: ClipSpec,
    out: &mut W,
) -> anyhow::Result<bool> {
    // ... (write_clipped 内容保持不变) ...
    // 为了节省篇幅，省略具体实现，直接复制你原本的逻辑即可
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let total = lines.len();
    if total == 0 { return Ok(false); }
    let ClipSpec { head, tail } = clip;
    if head + tail >= total {
        write!(out, "{}", content)?;
        return Ok(content.ends_with('\n'));
    }
    let mut ended_with_newline = false;
    let head_count = head.min(total);
    for l in &lines[..head_count] {
        write!(out, "{}", l)?;
        ended_with_newline = l.ends_with('\n');
    }
    let start_tail = total.saturating_sub(tail);
    let skipped = if start_tail > head_count { start_tail - head_count } else { 0 };
    if skipped > 0 {
        writeln!(out, "... (snipped {} lines) ...", skipped)?;
        ended_with_newline = true;
    }
    for l in &lines[start_tail..] {
        write!(out, "{}", l)?;
        ended_with_newline = l.ends_with('\n');
    }
    Ok(ended_with_newline)
}

fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ... (Logger struct and impl 保持不变) ...
#[derive(Clone)]
struct Logger {
    verbose: bool,
    quiet: bool,
}

impl Logger {
    fn new(verbose: bool, quiet: bool) -> Self {
        Self { verbose, quiet }
    }
    fn info(&self, msg: &str) {
        if self.quiet || !self.verbose { return; }
        eprintln!("{}", msg);
    }
    fn warn(&self, msg: &str) {
        if self.quiet { return; }
        eprintln!("{}", msg);
    }
    fn error(&self, msg: &str) {
        eprintln!("{}", msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn ext_match_is_case_insensitive() {
        assert!(ext_match(Path::new("foo.rs"), "rs,md"));
        assert!(ext_match(Path::new("foo.RS"), "rs,md"));
        assert!(!ext_match(Path::new("foo.txt"), "rs,md"));
        assert!(!ext_match(Path::new("foo"), "rs"));
    }

    #[test]
    fn should_use_textutil_recognizes_known_extensions() {
        assert!(should_use_textutil(Path::new("doc.DOCX")));
        assert!(should_use_textutil(Path::new("note.html")));
        assert!(!should_use_textutil(Path::new("note.txt")));
        assert!(!should_use_textutil(Path::new("noext")));
    }

    #[test]
    fn rel_display_strips_current_dir_prefix() {
        let cwd = std::env::current_dir().expect("cwd");
        let path = cwd.join("foo").join("bar.txt");
        assert_eq!(rel_display(&path, None), "foo/bar.txt");
    }

    #[test]
    fn strip_dot_slash_removes_prefix() {
        let path = Path::new("./nested/value");
        assert_eq!(strip_dot_slash(path), "nested/value");
    }

    #[test]
    fn rel_display_uses_custom_base() {
        let base = std::env::temp_dir().join("rel-display-base");
        let path = base.join("project/file.txt");
        assert_eq!(rel_display(&path, Some(&base)), "project/file.txt");
    }

    #[test]
    fn file_len_handles_missing_file() {
        let path = Path::new("unlikely_missing_file");
        assert!(file_len(path).unwrap().is_none());
    }

    #[test]
    fn binary_detection_by_null_byte() {
        assert!(is_probably_binary(b"abc\0def"));
        assert!(!is_probably_binary(b"plain text"));
    }

    #[test]
    fn write_clipped_inserts_snipped_line() {
        let content = "line1\nline2\nline3\nline4\nline5\nline6\n";
        let clip = ClipSpec { head: 2, tail: 2 };
        let mut buf = Vec::new();
        let ended = write_clipped(content, clip, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();

        assert!(s.contains("line1\nline2\n"));
        assert!(s.contains("line5\nline6\n"));
        assert!(s.contains("... (snipped 2 lines) ..."));
        assert!(ended);
    }
}