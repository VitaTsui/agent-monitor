import React, { useEffect, useMemo, useState } from "react";

import { message as antdMessage } from "@hsu-react/ui";
import { observer } from "mobx-react-lite";
import { Outlet, useLocation, useNavigate } from "react-router-dom";

import { claimPairDevice, getMe } from "@/services/apis/portal";
import { getAccessToken, getUserInfo, removeToken, setUserInfo } from "@/utils/auth";
import {
  clientSilentLogin,
  inDesktopClient,
  localMachineId,
} from "@/utils/clientAuth";
import { MOBILE_QUERY, isMobileViewport } from "@/utils/breakpoint";
import PortalStore from "./PortalStore";
import { useApkUpdateCheck } from "./_hooks/useApkUpdateCheck";
import { useClientUpdateToast } from "./_hooks/useClientUpdateToast";
import { useNativeBack } from "./_hooks/useNativeBack";
import { ShareReceiveModal } from "./_hooks/useShareReceive";
import MobileBar from "./_components/MobileBar";
import Sidebar from "./_components/Sidebar";
import { PortalUserContext, PortalUserInfo } from "./_context/portalUser";
import {
  PORTAL_BASE,
  SettingsTab,
  portalBackTarget,
  settingsPath,
} from "./_utils/portalNav";
import styles from "./index.module.scss";

/**
 * 前台的**壳**：移动端顶栏 ＋ 侧栏 ＋ 一个高度有界的内容区 ＋ `<Outlet />`。
 *
 * 档 B 之前这里是 786 行：侧栏、顶栏、会话网格、放大布局、用户菜单、设置弹窗
 * 全挤在一个组件里，右边显示什么由 `paneCount` 和 `settingsOpen` 这些 state 决定。
 * 代价是刷新丢失当前页、浏览器前进后退失效、「设备管理」这类界面发不出链接。
 *
 * 现在右边显示什么**由地址决定**（见 router.config.tsx 的 `/portal` 子路由），
 * 本组件只保留「所有子页面都需要的东西」：登录态自检、轮询生命周期、设备配对认领、
 * 更新提醒、抽屉开合、Android 返回键。
 */
const Portal: React.FC = observer(() => {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const { init, refresh, loadDevices, stopPolling, select } = PortalStore;

  // 客户端窗口内标出「本机」（浏览器里为 null，不标）
  const [localId, setLocalId] = useState<string | null>(null);
  const [siderFolded, setSiderFolded] = useState(false);
  const [userMenuOpen, setUserMenuOpen] = useState(false);
  // 移动端：侧栏抽屉开合
  const [mobileNav, setMobileNav] = useState(false);
  const [isMobile, setIsMobile] = useState(isMobileViewport);
  // 用 state 承载用户信息：每次加载调 /monitor/me 刷新（含实时 isSuper），
  // 这样管理员改了别人的权限，对方不必重新登录、下次加载即生效。
  const [user, setUser] = useState<PortalUserInfo>(
    () => (getUserInfo() as PortalUserInfo) ?? {},
  );

  // 会话页（index 路由）才显示移动端顶栏：设置 / 历史都自带固定头部，
  // 两个头叠一起就是双层导航栏
  const atSessions = pathname.replace(/\/+$/, "") === PORTAL_BASE;

  useEffect(() => {
    localMachineId().then((id) => {
      setLocalId(id);
      // 客户端窗口默认选中本机（用户手动切换过则不覆盖）
      if (id) {
        PortalStore.setLocalMachineId(id);
      }
    });
  }, []);

  // 移动端强制展开侧栏内容：桌面折叠态下缩窄窗口时，
  // CSS 会把抽屉撑到 84vw，但折叠态 JSX 不渲染内容 → 空白抽屉，这里在 JS 层纠正
  useEffect(() => {
    const mq = window.matchMedia(MOBILE_QUERY);
    const sync = () => {
      setIsMobile(mq.matches);
      if (mq.matches) {
        setSiderFolded(false);
      }
    };
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);

  // 本页完全响应式，豁免 index.scss 里给后管布局设的 min-width:960px
  // （窗口拖窄到 960 以下时那条会造成整页横滚、右上角控制键被推出可视区）
  useEffect(() => {
    document.body.setAttribute("data-fluid", "");
    return () => document.body.removeAttribute("data-fluid");
  }, []);

  // 抽屉打开时：锁背景滚动 + Esc 关闭
  useEffect(() => {
    if (!mobileNav) return;
    const prev = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setMobileNav(false);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      document.body.style.overflow = prev;
      window.removeEventListener("keydown", onKey);
    };
  }, [mobileNav]);

  // 前台需登录：无 token 时，客户端窗口先用设备令牌静默续登（客户端登录
  // 态永不过期），浏览器（或续登失败）才跳登录页并带回跳地址
  useEffect(() => {
    if (!getAccessToken()) {
      if (inDesktopClient()) {
        clientSilentLogin().then((ok) => {
          if (ok) {
            window.location.reload();
          } else {
            window.location.href = "/login?redirect=%2Fportal";
          }
        });
      } else {
        window.location.href = "/login?redirect=%2Fportal";
      }
      return;
    }
    init();

    // 刷新当前用户信息（含实时 isSuper）：改了权限无需重新登录，下次加载即生效
    getMe()
      .then((res) => {
        // 拦截器已保证是业务信封；这里只确认 data 确实是一条用户记录，
        // 不让形状不对的 payload 覆盖掉本地已有的登录用户信息。
        if (res.code === 0 && res.data?.username) {
          setUserInfo(res.data);
          setUser(res.data);
        }
      })
      .catch(() => void 0);

    return () => {
      stopPolling();
    };
  }, [init, stopPolling]);

  // 移动端更新推送：原生壳内检测 APK 新版本（浏览器里空转）
  useApkUpdateCheck();
  // 客户端窗口内右下角的新版本提醒（浏览器里空转）
  useClientUpdateToast();
  // 客户端窗口内：Cmd/Ctrl+R 刷新页面（webview 默认不绑，站点发新版可手动拉最新）
  useEffect(() => {
    if (!inDesktopClient()) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && (e.key === "r" || e.key === "R")) {
        e.preventDefault();
        window.location.reload();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 设备配对认领：客户端窗口带 ?pair=码 打开本页，登录后自动把那台电脑
  // 绑定到当前账号（绑定即信任），页面随即出现该设备 —— 用户零手工配置。
  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const code = params.get("pair");
    if (!code) return;
    // 无论成败都摘掉参数，避免刷新重复认领
    const clean = () => {
      params.delete("pair");
      const q = params.toString();
      window.history.replaceState(
        null,
        "",
        window.location.pathname + (q ? `?${q}` : ""),
      );
    };
    claimPairDevice(code)
      .then((res) => {
        if (res.code === 0) {
          antdMessage.success(
            `已绑定本机「${res.data?.hostname ?? ""}」到你的账号，终端会话马上出现`,
          );
          refresh();
          loadDevices();
        } else {
          antdMessage.warning(res.msg || "配对码无效或已过期，请重启客户端重试");
        }
      })
      .catch(() => antdMessage.error("绑定失败，请检查网络后重启客户端重试"))
      .finally(clean);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 原生壳（Android）的返回键：先关浮层，再退一层路由，都没有才交还系统语义。
  //
  // 「退一层」不能写成 `history.back()`：从推送/深链直接进 /portal/settings/devices 时
  // 历史里没有上一条，`canGoBack` 为 false，系统语义会直接退出整个 App。
  // 按地址算目标（portalBackTarget）才是确定的一层，且与页面里的返回按钮、
  // 浏览器后退落到同一个地方。浏览器里此钩子空转。
  useNativeBack(() => {
    if (mobileNav) {
      setMobileNav(false);
      return true;
    }
    if (userMenuOpen) {
      setUserMenuOpen(false);
      return true;
    }
    const target = portalBackTarget(pathname, isMobile);
    if (target) {
      navigate(target);
      return true;
    }
    return false;
  });

  // 移动端选中会话后自动收起抽屉；在子页面（设置/历史）里点会话则回会话页
  const selectSession = (id: string) => {
    select(id);
    setMobileNav(false);
    if (!atSessions) {
      navigate(PORTAL_BASE);
    }
  };

  const openSettings = (tab: SettingsTab) => {
    setUserMenuOpen(false);
    setMobileNav(false);
    navigate(settingsPath(tab));
  };

  const onLogout = () => {
    removeToken();
    window.location.href = "/login?redirect=%2Fportal";
  };

  const userValue = useMemo(() => user, [user]);

  if (!getAccessToken()) {
    return null;
  }

  // 本页不再自己给一份 antd 主题：主色 / 链接色 / 实心控件前景统一由 router/Routes.tsx
  // 的 ConfigProvider 按当前明暗下发（值来自 styles/primary.ts，与 tokens.scss 同步）。
  return (
    <PortalUserContext.Provider value={userValue}>
      <div className={styles.Portal}>
        {atSessions && (
          <MobileBar navOpen={mobileNav} onToggleNav={() => setMobileNav((v) => !v)} />
        )}

        {/* 移动端抽屉遮罩 */}
        {mobileNav && (
          <div className={styles.mobileBackdrop} onClick={() => setMobileNav(false)} />
        )}

        <Sidebar
          folded={siderFolded}
          onToggleFold={() => setSiderFolded(!siderFolded)}
          isMobile={isMobile}
          localId={localId}
          user={user}
          userMenuOpen={userMenuOpen}
          onUserMenuOpenChange={setUserMenuOpen}
          onSelectSession={selectSession}
          onOpenSettings={openSettings}
          onLogout={onLogout}
        />

        <main className={`${styles.main} ${mobileNav ? styles.mainPushed : ""}`}>
          <Outlet />
        </main>

        <ShareReceiveModal />
      </div>
    </PortalUserContext.Provider>
  );
});

export default Portal;
