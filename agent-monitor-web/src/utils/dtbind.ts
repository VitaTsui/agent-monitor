/**
 * 钉钉绑定 token 中转：钉钉机器人给未绑定的钉钉号回一条带 `?dtbind=<token>` 的登录链接。
 * 用户点开可能先被跳到登录页（查询参数会丢），故一进站就把 token 暂存下来，登录后再消费绑定。
 */
const KEY = "dtbind_token";

/** 一进站就从 URL 摘出 ?dtbind= 暂存，并把它从地址栏清掉（免登录跳转丢失 / 刷新重复绑定）。 */
export function stashDtbindToken(): void {
  try {
    const u = new URL(window.location.href);
    const t = u.searchParams.get("dtbind");
    if (t) {
      sessionStorage.setItem(KEY, t);
      u.searchParams.delete("dtbind");
      window.history.replaceState(null, "", u.toString());
    }
  } catch {
    /* 非浏览器环境或 URL 异常：忽略，绑定只是增强 */
  }
}

/** 取出并清除暂存的 token（登录后调一次，拿到就去绑定）。 */
export function consumeDtbindToken(): string | null {
  try {
    const t = sessionStorage.getItem(KEY);
    if (t) {
      sessionStorage.removeItem(KEY);
    }
    return t;
  } catch {
    return null;
  }
}
