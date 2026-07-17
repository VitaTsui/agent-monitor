import { createContext, useCallback, useContext, useMemo } from "react";

import { array_is_includes } from "hsu-utils";

export const PermissionsContent = createContext<{
  permissions: string[] | null;
}>({
  permissions: null,
});

export default function usePermissions(hasPermi?: string[]): {
  permissions: string[] | null;
  permitted: boolean;
  checkPermission: (permission: string[] | undefined) => boolean;
} {
  const permissions = useContext(PermissionsContent).permissions;

  const checkPermission = useCallback(
    (hasPermi: string[] | undefined) => {
      // 如果hasPermi或permissions为空，则认为有权限
      if (!hasPermi || !permissions) {
        return true;
      }

      // 如果hasPermi或permissions为空数组，则认为没有权限
      if (
        (Array.isArray(hasPermi) && !hasPermi.length) ||
        (Array.isArray(permissions) && !permissions.length)
      ) {
        return false;
      }

      // 如果hasPermi和permissions都不为空数组，则判断hasPermi是否在permissions中
      return array_is_includes(permissions, hasPermi);
    },
    [permissions]
  );

  const permitted = useMemo(() => {
    return checkPermission(hasPermi);
  }, [hasPermi, checkPermission]);

  return {
    permissions,
    permitted,
    checkPermission,
  };
}
