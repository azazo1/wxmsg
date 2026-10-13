//! 配置迁移.
//!
//! 约定: 配置里必须有 `version`; 缺失时按 0 处理, 然后逐级升到 [`CONFIG_VERSION`]。
//! 每次改结构就在这里加一个 `upgrade_from_vN`, 不要就地改老版本的解析逻辑。

use super::{CONFIG_VERSION, Config};

/// 迁移失败的原因.
pub type MigrateError = String;

/// 把配置文本解析并迁移到当前版本.
pub fn migrate(text: &str) -> Result<Config, MigrateError> {
    let mut config: Config = toml::from_str(text).map_err(|error| error.to_string())?;

    // 循环而不是顺序调用: 每一轮之后都重新判断版本, 于是新增一级迁移不需要改这里的结构。
    loop {
        match config.version {
            CONFIG_VERSION => return Ok(config),
            0 => config = upgrade_from_v0(config),
            other => {
                return Err(format!(
                    "配置版本 {other} 比程序支持的 {CONFIG_VERSION} 新, 请升级程序"
                ));
            }
        }
    }
}

/// 0 -> 1: 最早的配置没有 `version` 字段, 结构与 1 相同, 补上版本号即可.
///
/// 各字段都带 serde 默认值, 因此缺字段的旧配置在解析阶段就已经被补齐。
fn upgrade_from_v0(mut config: Config) -> Config {
    config.version = 1;
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_versionless_config() {
        let config = migrate("[paths]\nsource = \"/tmp/example\"\n").expect("应能迁移");
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.paths.source, "/tmp/example");
    }

    #[test]
    fn rejects_future_version() {
        let error = migrate("version = 99\n").expect_err("未来版本应当被拒绝");
        assert!(error.contains("99"), "错误信息里应当出现版本号: {error}");
    }
}
