import React, {
  Suspense,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

import PageLoading from "../PageLoading";
import { RouteType } from "../../router.config";
import { debounce } from "lodash";
import useBackTop from "@/hooks/useBackTop";
// 用具名导入，不要 `import KeepAlive from "react-activation"`。
// react-activation 只发 CJS（入口还用 `if (process.env.NODE_ENV)` 二选一地
// `module.exports = require(...)`），标准 ESM 互操作下「默认导入」拿到的是整个
// module.exports 对象，不是 exports.default —— webpack 会读 __esModule 帮你退到
// .default，Vite / Node 不会。写成默认导入的话这里拿到的是个对象，React 直接抛
// 「Element type is invalid … got: object」，整个后管白屏。
import { KeepAlive } from "react-activation";
import { useLocation, useNavigate } from "react-router";
import { array_is_includes } from "hsu-utils";
import { getAccessToken } from "@/utils/auth";
import { ReloadContent, useReload } from "@hsu-react/ui/es/layout";
import useDocTitle from "@/router/_hooks/useDocTitle";
import { usePermissions } from "@hsu-react/ui";

const RouterContainer: React.FC<RouteType> = (props) => {
  const { meta = {}, element, path = "", children, index } = props;
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const { title, name, noCache, noLazy, noAuth, hasPermi } = meta;
  const { id: reLoadId } = useContext(ReloadContent);
  const [isReload, setIsReload] = useState(false);
  const { backTop } = useBackTop();
  const onReload = useReload();
  const { permitted } = usePermissions(hasPermi);

  useDocTitle(path, title || name);

  useEffect(() => {
    debounce(backTop)();
  }, [backTop]);

  const id = useMemo(() => {
    const pathArr = pathname.split("/").filter(Boolean);
    const keyArr = (index ? pathname : path).split("/").filter(Boolean);

    if (
      pathArr.length === keyArr.length &&
      array_is_includes(
        keyArr.filter((i) => !i.startsWith(":")),
        pathArr
      )
    ) {
      return pathname;
    }

    return path;
  }, [index, path, pathname]);

  useEffect(() => {
    if (id === reLoadId && reLoadId) {
      setIsReload(true);

      setTimeout(() => {
        setIsReload(false);
        onReload("");
      }, 0);
    }
  }, [id, onReload, reLoadId]);

  useEffect(() => {
    if (!noAuth && !getAccessToken()) {
      navigate("/login");
    }
  }, [navigate, noAuth]);

  if (!element || !permitted || isReload) {
    return null;
  }

  if (noLazy) {
    if (noCache) {
      return element;
    }

    if (!id) {
      return <PageLoading />;
    }

    return (
      <KeepAlive name={id} id={id}>
        {element}
      </KeepAlive>
    );
  }

  if (noCache || !!children?.length) {
    return <Suspense fallback={<PageLoading />}>{element}</Suspense>;
  }

  if (!id) {
    return <PageLoading />;
  }

  return (
    <KeepAlive name={id} id={id}>
      <Suspense fallback={<PageLoading />}>{element}</Suspense>
    </KeepAlive>
  );
};

export default RouterContainer;
