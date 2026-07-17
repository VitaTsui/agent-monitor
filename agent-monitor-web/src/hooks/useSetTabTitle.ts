import { createContext, useContext, useCallback } from "react";

export const NavTabBarTitleContent = createContext<{
  tabTitles?: Record<string, React.ReactNode>;
  setTabTitle?: (key: string, title: React.ReactNode) => void;
}>({});

export default function useSetTabTitle() {
  const { setTabTitle: _setTabTitle } = useContext(NavTabBarTitleContent);

  const setTabTitle = useCallback(
    (key: string, title: React.ReactNode) => {
      _setTabTitle?.(key, title);
    },
    [_setTabTitle]
  );

  return setTabTitle;
}
