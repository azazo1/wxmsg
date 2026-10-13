//! 与文件系统打交道时的公共小工具.

use std::io;

/// 重试被信号打断的系统调用.
///
/// 微信在写库, 系统时不时会送来信号, 于是 `read` 或 `read_dir` 会以
/// [`io::ErrorKind::Interrupted`] 返回。这不是错误, 直接重试即可; 长跑的任务 (归档, 找回)
/// 如果把它当成致命错误, 就会莫名其妙地半路退出。
pub fn retry<T>(mut action: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match action() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn retries_until_success() {
        let attempts = Cell::new(0);
        let value = retry(|| {
            attempts.set(attempts.get() + 1);
            if attempts.get() < 3 {
                Err(io::Error::from(io::ErrorKind::Interrupted))
            } else {
                Ok(attempts.get())
            }
        })
        .expect("第三次应当成功");
        assert_eq!(value, 3);
    }

    #[test]
    fn gives_up_on_other_errors() {
        let attempts = Cell::new(0);
        let error = retry::<()>(|| {
            attempts.set(attempts.get() + 1);
            Err(io::Error::from(io::ErrorKind::NotFound))
        })
        .expect_err("非中断错误应当直接返回");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(attempts.get(), 1);
    }
}
