import { useActivate } from "react-activation";
import { useLocation } from "react-router-dom";
import { useState, useEffect } from "react";

function parseSearch<T>(search: string) {
  const sp = new URLSearchParams(search);
  const obj: Record<string, unknown> = {};
  for (const key of sp.keys()) obj[key] = sp.get(key);
  return obj as T;
}

export default function useSearch<T extends Record<string, unknown>>(): T {
  const location = useLocation();
  const [value, setValue] = useState<T>(() => parseSearch<T>(location.search));

  // 组件“在路由树里且会更新”时：正常跟随 search 变化
  useEffect(() => {
    setValue(parseSearch<T>(location.search));
  }, [location.search]);

  // 从 KeepAlive 缓存“激活回来”时：再同步一次（关键）
  useActivate(() => {
    setValue(parseSearch<T>(window.location.search));
  });

  return value;
}
