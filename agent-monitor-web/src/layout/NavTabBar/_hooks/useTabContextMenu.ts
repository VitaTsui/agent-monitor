import { useState } from "react";
import { useDebounceEffect } from "ahooks";

/**
 * 处理标签页右键菜单的 hook
 */
export const useTabContextMenu = () => {
  const [open, setOpen] = useState<string>("");

  useDebounceEffect(() => {
    const close = () => {
      setOpen("");
    };

    document.removeEventListener("click", close);
    if (open) {
      document.addEventListener("click", close);
    }
  }, [open]);

  return { open, setOpen };
};

