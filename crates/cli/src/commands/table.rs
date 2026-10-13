//! 终端文本表格.
//!
//! 只处理对齐与截断两件事: 中文按两列宽计算, 过长的正文按显示宽度截断。
//! 不用第三方表格库, 因为这里只需要固定几列的对齐输出。

/// 单个字符占用的显示列数.
fn char_width(ch: char) -> usize {
    let code = ch as u32;
    // 组合用符号与零宽字符不占位。
    if (0x0300..=0x036f).contains(&code) || code == 0x200d {
        return 0;
    }
    let wide = matches!(code,
        0x1100..=0x115f        // 谚文字母
        | 0x2e80..=0x303e      // CJK 部首, 康熙部首, CJK 符号
        | 0x3041..=0x33ff      // 假名, CJK 兼容
        | 0x3400..=0x4dbf      // CJK 扩展 A
        | 0x4e00..=0x9fff      // CJK 基本区
        | 0xa000..=0xa4cf      // 彝文
        | 0xac00..=0xd7a3      // 谚文音节
        | 0xf900..=0xfaff      // CJK 兼容表意
        | 0xfe30..=0xfe6f      // CJK 兼容形式
        | 0xff00..=0xff60      // 全角形式
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1f9ff    // 表情
        | 0x20000..=0x3fffd    // CJK 扩展 B 及以上
    );
    if wide { 2 } else { 1 }
}

/// 字符串的显示宽度.
pub(crate) fn width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// 按显示宽度截断, 超出部分用省略号表示.
pub(crate) fn truncate(text: &str, limit: usize) -> String {
    if width(text) <= limit {
        return text.to_owned();
    }
    let ellipsis = "...";
    let budget = limit.saturating_sub(ellipsis.len());
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let next = used + char_width(ch);
        if next > budget {
            break;
        }
        out.push(ch);
        used = next;
    }
    out.push_str(ellipsis);
    out
}

/// 右对齐补齐到指定显示宽度.
fn pad(text: &str, target: usize, right_align: bool) -> String {
    let filler = target.saturating_sub(width(text));
    let spaces = " ".repeat(filler);
    if right_align {
        format!("{spaces}{text}")
    } else {
        format!("{text}{spaces}")
    }
}

/// 渲染一张表格, 列间以两个空格分隔, 最后一列不补空格.
///
/// `right_align` 与 `headers` 等长, 用于标记哪些列右对齐 (通常是数字列)。
pub(crate) fn render(headers: &[&str], right_align: &[bool], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .filter_map(|row| row.get(index))
                .map(|cell| width(cell))
                .chain(std::iter::once(width(header)))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let line = |cells: &[String]| -> String {
        let mut out = String::new();
        for (index, cell) in cells.iter().enumerate() {
            if index > 0 {
                out.push_str("  ");
            }
            let align_right = right_align.get(index).copied().unwrap_or(false);
            let last = index + 1 == cells.len();
            if last {
                out.push_str(cell);
            } else {
                out.push_str(&pad(cell, widths[index], align_right));
            }
        }
        out.trim_end().to_owned()
    };

    let header_cells: Vec<String> = headers.iter().map(|text| (*text).to_owned()).collect();
    let mut out = line(&header_cells);
    out.push('\n');
    out.push_str(&line(
        &widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>(),
    ));
    for row in rows {
        out.push('\n');
        out.push_str(&line(row));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_cjk_as_two_columns() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("微信"), 4);
        assert_eq!(width("微信a"), 5);
    }

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate("abcdef", 10), "abcdef");
        assert_eq!(truncate("abcdef", 5), "ab...");
        assert_eq!(truncate("中文中文", 5), "中...");
    }

    #[test]
    fn aligns_columns() {
        let text = render(
            &["行号", "内容"],
            &[true, false],
            &[
                vec!["1".into(), "短".into()],
                vec!["100".into(), "长一点".into()],
            ],
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        // 第一列宽度由最宽的表头 "行号" (4 列) 决定, 数字右对齐。
        assert_eq!(lines[0], "行号  内容");
        assert_eq!(lines[1], "----  ------");
        assert_eq!(lines[2], "   1  短");
        assert_eq!(lines[3], " 100  长一点");
    }
}
