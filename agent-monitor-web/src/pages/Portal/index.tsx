import React, { Suspense, lazy, useEffect, useMemo, useState } from "react";

import { message as antdMessage } from "@hsu-react/ui";
import { observer } from "mobx-react-lite";
import { Outlet, useLocation, useNavigate } from "react-router-dom";

import { claimPairDevice, getMe } from "@/services/apis/portal";
import {
  getAccessToken,
  getUserInfo,
  removeToken,
  setUserInfo,
} from "@/utils/auth";
import {
  clientSilentLogin,
  inDesktopClient,
  localMachineId,
} from "@/utils/clientAuth";
import PortalStore from "./PortalStore";
import { useApkUpdateCheck } from "./_hooks/useApkUpdateCheck";
import { useClientUpdateToast } from "./_hooks/useClientUpdateToast";
import { useIsMobile } from "./_hooks/useIsMobile";
import { useNativeBack } from "./_hooks/useNativeBack";
import { ShareReceiveModal } from "./_hooks/useShareReceive";
import MobileBar from "./_components/MobileBar";
import RightPane from "./_components/RightPane";
import SearchPalette from "./_components/SearchPalette";
import SessionStatePane from "./_components/SessionStatePane";
import Sidebar from "./_components/Sidebar";
import { PortalUserContext, PortalUserInfo } from "./_context/portalUser";
import { PORTAL_BASE, SettingsTab, portalBackTarget } from "./_utils/portalNav";
import styles from "./index.module.scss";

// 设置弹窗整块懒加载：里面的配置同步 / 机器人接入 / 设备管理都是首屏用不到的重块
const SettingsModal = lazy(() => import("./_components/SettingsModal"));

/**
 * 前台的**壳**：移动端顶栏 ＋ 侧栏 ＋ 一个高度有界的内容区 ＋ `<Outlet />`。
 *
 * 这里曾经是 786 行：侧栏、顶栏、会话网格、放大布局、用户菜单、设置弹窗全挤在
 * 一个组件里。现在侧栏 / 顶栏 / 会话网格 / 用户菜单 / 设置各自成组件或视图。
 *
 * 现在会话网格与远程往来**由地址决定**（见 router.config.tsx 的 `/portal` 子路由），
 * 本组件只保留「所有子页面都需要的东西」：登录态自检、轮询生命周期、设备配对认领、
 * 更新提醒、抽屉开合、设置弹窗开合、Android 返回键。
 *
 * 设置是**弹窗**不是地址：它是一次性动作（改完就走），不需要发链接、也不需要
 * 前进后退。代价是刷新会丢当前分栏、浏览器后退关不掉它 —— 这是明确取舍。
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
  const isMobile = useIsMobile();
  // 设置弹窗：开合与当前分栏。分栏放在壳里而不是弹窗内部 —— 返回键要靠它
  // 判断「退到一级菜单」还是「关掉弹窗」，两处各存一份就会不同步。
  // `null` = 移动端停在一级菜单（桌面无此态，弹窗按「账户」渲染）。
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  // 首次打开后就不再卸载：卸载会掐掉 antd 的关闭动画（移动端是底部 sheet，更明显）
  const [settingsMounted, setSettingsMounted] = useState(false);
  // 命令面板（⌘K）。开合放在壳上而不是面板里 —— 面板没开的时候它自己不在树上，
  // 监听不到任何键；侧栏那颗搜索按钮也要能开它。
  const [searchOpen, setSearchOpen] = useState(false);
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
    if (isMobile) {
      setSiderFolded(false);
    }
  }, [isMobile]);

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

  /* 换页就收起抽屉。
   *
   * 移动端的抽屉是「推开内容」式的：开着的时候整块内容被右移 320px。此前每一处
   * 会导航的入口（选会话、开设置）都各自手写了一次 `setMobileNav(false)`，
   * 「换了页要收起来」从来不是一条规则、而是三处巧合 —— 侧栏新增「查看全部会话」
   * 这个入口时立刻现形：跳过去了，抽屉还开着，新页面被推在屏幕外。
   * 收成一条 effect，之后再加入口就不必记得补这一行。 */
  useEffect(() => {
    setMobileNav(false);
  }, [pathname]);

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
  /* 全局命令面板：⌘K / Ctrl+K 开，Esc 关（Esc 由 Modal 自己接）。
   *
   * 带修饰键，所以在输入框（对话框、备注、搜索行）里聚焦时也不会被当成正常输入误触发；
   * `preventDefault` 挡掉浏览器自己的 ⌘K（Chrome 聚焦地址栏搜索）。
   * 挂 window 上是因为入口有三个：这个键、侧栏头部那颗按钮、移动端顶栏那颗。 */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "k") {
        e.preventDefault();
        // **开关**，不是只管开：同一个键既然能唤起它，就得能收回去。只管开的话，
        // 一旦焦点不在弹窗里（Esc 走的是弹窗自己的按键通道），这个遮罩就没有
        // 键盘出路了 —— 页面点不动、Esc 又不响应，只能刷新。
        setSearchOpen((v) => !v);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

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
          antdMessage.warning(
            res.msg || "配对码无效或已过期，请重启客户端重试",
          );
        }
      })
      .catch(() => antdMessage.error("绑定失败，请检查网络后重启客户端重试"))
      .finally(clean);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 原生壳（Android）的返回键：从内到外一层层退 —— 抽屉 → 用户菜单 →
  // 设置（移动端先退到一级菜单，再关弹窗）→ 退一层路由，都没有才交还系统语义。
  //
  // 「退一层路由」不能写成 `history.back()`：从推送/深链直接进 /portal/history/xxx 时
  // 历史里没有上一条，`canGoBack` 为 false，系统语义会直接退出整个 App。
  // 按地址算目标（portalBackTarget）才是确定的一层。浏览器里此钩子空转。
  useNativeBack(() => {
    if (searchOpen) {
      setSearchOpen(false);
      return true;
    }
    if (mobileNav) {
      setMobileNav(false);
      return true;
    }
    if (userMenuOpen) {
      setUserMenuOpen(false);
      return true;
    }
    if (settingsOpen) {
      // 移动端是 iOS 式两级：二级分栏先退回一级菜单，一级菜单才关弹窗
      if (isMobile && settingsTab) {
        setSettingsTab(null);
      } else {
        setSettingsOpen(false);
      }
      return true;
    }
    const target = portalBackTarget(pathname);
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
    setSettingsMounted(true);
    setSettingsTab(tab);
    setSettingsOpen(true);
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
          <MobileBar
            navOpen={mobileNav}
            onToggleNav={() => setMobileNav((v) => !v)}
            onOpenSearch={() => setSearchOpen(true)}
          />
        )}

        {/* 移动端抽屉遮罩 */}
        {mobileNav && (
          <div
            className={styles.mobileBackdrop}
            onClick={() => setMobileNav(false)}
          />
        )}

        <Sidebar
          folded={siderFolded}
          onToggleFold={() => setSiderFolded(!siderFolded)}
          isMobile={isMobile}
          underTopBar={atSessions}
          localId={localId}
          user={user}
          userMenuOpen={userMenuOpen}
          onUserMenuOpenChange={setUserMenuOpen}
          onSelectSession={selectSession}
          onOpenSettings={openSettings}
          onOpenSearch={() => setSearchOpen(true)}
          onLogout={onLogout}
        />

        {/* 命令面板。它只是**现有入口的另一种打开方式**：会话、设备、
            「查看全部会话」、设置各分栏、外观三态，逐条都能在界面上找到对应的地方。
            侧栏原来那个筛当前设备的搜索框已经撤掉，不留两套搜索。 */}
        <SearchPalette
          open={searchOpen}
          onClose={() => setSearchOpen(false)}
          onSelectSession={selectSession}
          onOpenSettings={openSettings}
        />

        {/* 正文 ＋ 右栏并排的那一行。右栏给的条件有三条：
            **会话页** —— 设置/历史都是自带固定头部的整页内容，右边再钉一栏会话
              状态既对不上也没地方摆；
            **宽屏** —— 一块 390 宽的屏摆不下第三栏，状态卡退回对话流末尾
              （见 ChatPane）；
            **至少开着一格** —— 一格没开时那栏没有主语，摆一条「暂无」在空问候语
              旁边只是白占三成宽。 */}
        <div
          className={`${styles.contentRow} ${mobileNav ? styles.mainPushed : ""}`}
        >
          <main className={styles.main}>
            <Outlet />
          </main>

          {atSessions &&
            !isMobile &&
            PortalStore.rightPaneOpen &&
            PortalStore.openTasks.length > 0 && (
              <RightPane>
                <SessionStatePane />
              </RightPane>
            )}
        </div>

        {settingsMounted && (
          <Suspense fallback={null}>
            <SettingsModal
              open={settingsOpen}
              tab={settingsTab}
              isMobile={isMobile}
              onTabChange={setSettingsTab}
              onClose={() => setSettingsOpen(false)}
            />
          </Suspense>
        )}

        <ShareReceiveModal />
      </div>
    </PortalUserContext.Provider>
  );
});

export default Portal;
