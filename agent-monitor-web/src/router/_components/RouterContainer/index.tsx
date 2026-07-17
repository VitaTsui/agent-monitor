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
import KeepAlive from "react-activation";
import { useLocation, useNavigate } from "react-router";
import { array_is_includes } from "hsu-utils";
import { getAccessToken } from "@/utils/auth";
import { ReloadContent } from "@/hooks/useReload";
import useReload from "@/hooks/useReload";
import useDocTitle from "@/router/_hooks/useDocTitle";
import usePermissions from "@/hooks/usePermissions";

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
