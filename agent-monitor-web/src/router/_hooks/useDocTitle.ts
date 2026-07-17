import { array_is_includes } from "hsu-utils";
import { useEffect } from "react";
import { useLocation } from "react-router";

export default function useDocTitle(
  path?: string,
  title?: string,
  only?: boolean
) {
  const { pathname } = useLocation();

  useEffect(() => {
    setTimeout(() => {
      const pathArr = pathname.split("/").filter(Boolean);
      const keyArr = path?.split("/").filter(Boolean) || [];

      if (
        (keyArr.length > 0 &&
          pathArr.length === keyArr.length &&
          (array_is_includes(keyArr, pathArr) ||
            array_is_includes(
              keyArr.filter((i) => !i.startsWith(":")),
              pathArr
            ))) ||
        (pathname === "/" && path === "/")
      ) {
        if (title) {
          if (Config.title && !only) {
            document.title = `${Config.title} - ${title}`;
          } else {
            document.title = title;
          }
        } else {
          document.title = Config.title || document.title;
        }
      }
    }, 0);
  }, [only, path, pathname, title]);
}
