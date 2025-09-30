use base64::engine::general_purpose::STANDARD as Base64;
use base64::Engine;
use clap::{Parser, ValueEnum};
use globwalk::GlobWalkerBuilder;
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

#[derive(Debug, Parser)]
#[command(
    name = "printfiles",
    version,
    about = "Print files matched by globs/dirs with ===header=== and ===end of 'file'==="
)]
struct Args {
    /// 一组以空格或逗号分隔的模式或目录，例如：
    /// "src/**/*.rs,docs/*.md" tests "README*"
    #[arg(required = true)]
    items: Vec<String>,

    /// 读取后端：text(默认) / textutil / auto
    #[arg(long, value_enum, default_value_t = Reader::Text)]
    reader: Reader,

    /// 若传入目录，是否仅限这些扩展（逗号分隔，如 "rs,md"）。
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

    /// 排序策略：按名称、大小或修改时间
    #[arg(long, value_enum, default_value_t = SortKey::Name)]
    sort: SortKey,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SortKey {
    Name,
    Size,
    Mtime,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let relative_base = resolve_relative_base(args.relative_from.as_ref())?;

    // 解析 items：支持空格和逗号混合
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
        eprintln!("（未匹配到任何文件）");
        std::process::exit(2);
    }

    // 收集匹配到的文件到有序集合，确保稳定输出且去重
    let mut files: BTreeSet<PathBuf> = BTreeSet::new();

    for token in tokens {
        let path = Path::new(&token);
        if path.is_dir() {
            // 目录：递归匹配所有文件，或受 --ext 限制
            if let Err(err) = collect_dir(path, args.ext.as_deref(), &mut files) {
                eprintln!("警告: 目录遍历失败 {token}: {err}");
            }
            continue;
        }

        match expand_glob(&token) {
            Ok(paths) => {
                for path in paths {
                    if path.is_file() {
                        files.insert(normalize(&path));
                    }
                }
            }
            Err(err) => {
                eprintln!("警告: 模式无效或没有匹配: {err}");
            }
        }
    }

    if files.is_empty() {
        eprintln!("（未匹配到任何文件）");
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
        writeln!(out, "==={}===", rel)?;
        if let Some(limit) = args.max_size {
            if let Some(size) = entry.len {
                if size > limit {
                    eprintln!(
                        "提示: 跳过 {} (size={} > max_size={})",
                        path.display(),
                        size,
                        limit
                    );
                    writeln!(out, "(skipped: file exceeds max size)")?;
                    writeln!(out, "===end of '{}'===", rel)?;
                    continue;
                }
            }
        }

        if let Err(err) = read_and_write(&path, args.reader, args.binary, &mut out) {
            eprintln!("错误: 读取失败 {}: {err}", path.display());
            had_error = true;
        }
        writeln!(out, "===end of '{}'===", rel)?;
    }

    out.flush()?;
    if had_error {
        std::process::exit(1);
    }

    Ok(())
}

fn collect_dir(
    dir: &Path,
    exts: Option<&str>,
    files: &mut BTreeSet<PathBuf>,
) -> anyhow::Result<()> {
    let walker = GlobWalkerBuilder::from_patterns(dir, &["**/*"])
        .follow_links(true)
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
    // 保持路径相对性，不做 canonicalize，以免跨文件系统/权限问题
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

fn expand_glob(pattern: &str) -> anyhow::Result<Vec<PathBuf>> {
    let has_glob = pattern.contains('*') || pattern.contains('?') || pattern.contains('[');
    if !has_glob {
        return Ok(vec![PathBuf::from(pattern)]);
    }

    let walker = GlobWalkerBuilder::from_patterns(".", &[pattern])
        .follow_links(true)
        .case_insensitive(false)
        .build()?;
    Ok(walker
        .filter_map(|e| e.ok())
        .map(|e| e.path().to_path_buf())
        .collect())
}

fn read_and_write<W: Write>(
    path: &Path,
    reader: Reader,
    binary: BinaryStrategy,
    mut out: W,
) -> anyhow::Result<()> {
    match reader {
        Reader::Text => write_text(path, binary, &mut out),
        Reader::Textutil => write_textutil_then_fallback(path, binary, &mut out),
        Reader::Auto => {
            if should_use_textutil(path) {
                write_textutil_then_fallback(path, binary, &mut out)
            } else {
                write_text(path, binary, &mut out)
            }
        }
    }
}

fn write_text<W: Write>(path: &Path, binary: BinaryStrategy, out: &mut W) -> anyhow::Result<()> {
    match fs::read(path) {
        Ok(bytes) => {
            if handle_binary(path, &bytes, binary, out)? {
                return Ok(());
            }
            // 尽量用 UTF-8 显示，非 UTF-8 时采用有损转换
            let s = String::from_utf8_lossy(&bytes);
            write!(out, "{}", s)?;
            Ok(())
        }
        Err(e) => {
            anyhow::bail!("{}", e);
        }
    }
}

fn write_textutil_then_fallback<W: Write>(
    path: &Path,
    binary: BinaryStrategy,
    out: &mut W,
) -> anyhow::Result<()> {
    if which::which("textutil").is_ok() {
        let output = Command::new("textutil")
            .arg("-convert")
            .arg("txt")
            .arg("-stdout")
            .arg(path)
            .output();
        match output {
            Ok(outp) if outp.status.success() => {
                out.write_all(&outp.stdout)?;
                return Ok(());
            }
            Ok(outp) => {
                eprintln!(
                    "警告: textutil 处理失败 ({}), 回退到文本读取: {}",
                    outp.status,
                    path.display()
                );
            }
            Err(e) => {
                eprintln!(
                    "警告: textutil 调用异常 ({}), 回退到文本读取: {}",
                    e,
                    path.display()
                );
            }
        }
    } else {
        eprintln!(
            "提示: 未检测到 textutil，回退到文本读取。文件: {}",
            path.display()
        );
    }
    write_text(path, binary, out)
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

fn handle_binary<W: Write>(
    path: &Path,
    bytes: &[u8],
    strategy: BinaryStrategy,
    out: &mut W,
) -> anyhow::Result<bool> {
    if !is_probably_binary(bytes) || matches!(strategy, BinaryStrategy::Print) {
        return Ok(false);
    }

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
    eprintln!("提示: 二进制文件按 {:?} 处理: {}", strategy, path.display());
    Ok(true)
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
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
}
