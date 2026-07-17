import { createContext, useContext, useCallback } from "react";

export const NavTabBarContent = createContext<{
  dropKey?: string;
  setDropKey?: (key: string) => void;
}>({});

export default function useDropTab() {
  const { setDropKey } = useContext(NavTabBarContent);

  const drop = useCallback(
    (key: string) => {
      setDropKey?.(key);

      setTimeout(() => {
        setDropKey?.("");
      }, 0);
    },
    [setDropKey]
  );

  return drop;
}
