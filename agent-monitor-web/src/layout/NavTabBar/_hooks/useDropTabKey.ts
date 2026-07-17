import { useEffect, useContext } from "react";
import { useAliveController } from "react-activation";
import { NavTabBarContent } from "@/hooks/useDropTab";
import { TabType } from "..";

/**
 * 处理通过 dropKey 关闭标签页的 hook
 */
export const useDropTabKey = (
  setOpenkeys: React.Dispatch<React.SetStateAction<TabType[]>>
) => {
  const { drop } = useAliveController();
  const { dropKey } = useContext(NavTabBarContent);

  useEffect(() => {
    setTimeout(() => {
      if (dropKey) {
        drop(dropKey);

        setOpenkeys((prev) => {
          const newOpenKeys = prev.filter((item) => item.key !== dropKey);
          return newOpenKeys;
        });
      }
    }, 0);
  }, [drop, dropKey, setOpenkeys]);
};
