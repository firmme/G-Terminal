//! Developer-only live OAuth diagnostic. Uses the normal application's login
//! code/keychain entry. It never reads, uploads or changes saved connections.
use std::sync::atomic::AtomicBool;

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--refresh-only") {
        g_terminal::sync::check_login()?;
        println!("刷新登录令牌成功。");
        return Ok(());
    }
    println!("此诊断只验证登录，成功后凭据保存到 G-Terminal 的系统凭据项，不同步连接列表。");
    g_terminal::sync::login(
        |url| {
            println!("请在浏览器打开以下授权地址：\n{url}");
            Ok(())
        },
        &AtomicBool::new(false),
    )?;
    println!("授权码交换成功，登录凭据已保存到系统凭据库。");
    g_terminal::sync::check_login()?;
    println!("刷新登录令牌成功。");
    Ok(())
}
