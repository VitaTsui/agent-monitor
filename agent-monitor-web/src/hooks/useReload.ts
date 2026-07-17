import { createContext, useCallback, useContext } from "react";

export const ReloadContent = createContext<{
  id?: string;
  setId?: (id: string) => void;
}>({});

export default function useReload() {
  const { setId: setReLoadId } = useContext(ReloadContent);

  const onReload = useCallback(
    (id: string) => {
      setReLoadId?.(id);
    },
    [setReLoadId]
  );

  return onReload;
}
