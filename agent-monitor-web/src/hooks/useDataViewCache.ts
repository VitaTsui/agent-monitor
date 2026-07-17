import { useEffect, useState, useCallback, useRef } from "react";
import { useLocation } from "react-router-dom";
import { ColumnsType, FormItemProps } from "@hsu-react/ui";
import wsCache from "@/utils/wsCache";

interface ColumnCacheData {
  dataIndex: string;
  hidden: boolean;
  width?: number;
  ellipsis?: boolean;
  sort?: number;
}

interface CacheData {
  searchItemsVisible?: Array<{ name: string; visible: boolean }>;
  columnsConfig?: Array<ColumnCacheData>;
}

interface UseDataViewCacheOptions {
  /** 缓存 key 前缀，默认为 'dataViewSearch' */
  cacheKeyPrefix?: string;
  /** 初始搜索项 */
  initialSearchItems: FormItemProps[];
  /** 初始列配置 */
  initialColumns: ColumnsType;
  /** 是否启用缓存，默认为 true */
  enabled?: boolean;
}

interface UseDataViewCacheReturn {
  /** 搜索项（带缓存状态） */
  searchItems: FormItemProps[];
  /** 列配置（带缓存状态） */
  columns: ColumnsType;
  /** 设置搜索项 */
  setSearchItems: (items: FormItemProps[]) => void;
  /** 设置列配置 */
  setColumns: (columns: ColumnsType) => void;
  /** 处理搜索项过滤变化 */
  handleSearchFilterChange: (items: FormItemProps[]) => void;
  /** 处理列选择变化 */
  handleColumnSelectionChange: (
    selectedDataIndexes: string[],
    dataSource: Array<{
      dataIndex: string;
      hidden: boolean;
      width?: number;
      ellipsis?: boolean;
      sort?: number;
    }>
  ) => void;
}

/**
 * 数据视图缓存 Hook
 * 用于管理列表显示项和查询勾选项的缓存
 */
export default function useDataViewCache(
  options: UseDataViewCacheOptions
): UseDataViewCacheReturn {
  const {
    cacheKeyPrefix = "dataViewSearch",
    initialSearchItems,
    initialColumns,
    enabled = true,
  } = options;

  const { pathname } = useLocation();
  const cacheKey = `${cacheKeyPrefix}_${pathname}`;

  const [searchItems, setSearchItemsState] = useState<FormItemProps[]>([]);
  const [columns, setColumnsState] = useState<ColumnsType>([]);
  const prevColumnsRef = useRef<string>("");

  // 从缓存恢复状态
  useEffect(() => {
    if (!enabled) {
      setSearchItemsState(initialSearchItems);
      setColumnsState(initialColumns);
      return;
    }

    const cachedData: CacheData | null = wsCache.get(cacheKey);

    // 恢复搜索项
    if (cachedData?.searchItemsVisible) {
      const restoredSearchItems = initialSearchItems?.map((item) => {
        const cachedItem = cachedData.searchItemsVisible?.find(
          (cached) => cached.name === item.name
        );
        return {
          ...item,
          visible: cachedItem ? cachedItem.visible : item.visible,
        };
      });
      setSearchItemsState(restoredSearchItems);
    } else {
      setSearchItemsState(initialSearchItems);
    }

    // 恢复列配置
    if (cachedData?.columnsConfig) {
      const restoredColumns = initialColumns?.map((col) => {
        const cachedCol = cachedData.columnsConfig?.find(
          (cached) => cached.dataIndex === col.dataIndex
        );
        if (cachedCol) {
          return {
            ...col,
            hidden: cachedCol.hidden,
            width: cachedCol.width !== undefined ? cachedCol.width : col.width,
            ellipsis:
              cachedCol.ellipsis !== undefined
                ? cachedCol.ellipsis
                : col.ellipsis,
            sort: cachedCol.sort !== undefined ? cachedCol.sort : col.sort,
          };
        }
        return col;
      });
      // 按 sort 排序
      restoredColumns.sort((a, b) => (a.sort ?? 0) - (b.sort ?? 0));
      setColumnsState(restoredColumns);
    } else {
      setColumnsState(initialColumns);
    }
  }, [cacheKey, enabled, initialSearchItems, initialColumns]);

  // 设置搜索项（带缓存保存）
  const setSearchItems = useCallback(
    (items: FormItemProps[]) => {
      setSearchItemsState(items);
      if (enabled) {
        const searchItemsVisible = items?.map((item) => ({
          name: item.name,
          visible: item.visible !== false,
        }));
        const cachedData: CacheData = wsCache.get(cacheKey) || {};
        wsCache.set(cacheKey, {
          ...cachedData,
          searchItemsVisible,
        });
      }
    },
    [cacheKey, enabled]
  );

  // 设置列配置（带缓存保存）
  const setColumns = useCallback(
    (newColumns: ColumnsType) => {
      setColumnsState(newColumns);
      if (enabled && newColumns.length > 0) {
        const columnsConfig: ColumnCacheData[] = newColumns
          .filter((col) => col.dataIndex) // 过滤掉没有 dataIndex 的列
          ?.map((col) => ({
            dataIndex: col.dataIndex!,
            hidden: col.hidden || false,
            width: typeof col.width === "number" ? col.width : undefined,
            ellipsis:
              typeof col.ellipsis === "boolean" ? col.ellipsis : undefined,
            sort: typeof col.sort === "number" ? col.sort : undefined,
          }));
        const currentColumnsKey = JSON.stringify(columnsConfig);

        // 只有当配置真正改变时才更新缓存
        if (currentColumnsKey !== prevColumnsRef.current) {
          const cachedData: CacheData = wsCache.get(cacheKey) || {};
          wsCache.set(cacheKey, {
            ...cachedData,
            columnsConfig,
          });
          prevColumnsRef.current = currentColumnsKey;
        }
      }
    },
    [cacheKey, enabled]
  );

  // 处理搜索项过滤变化
  const handleSearchFilterChange = useCallback(
    (items: FormItemProps[]) => {
      setSearchItems(items);
    },
    [setSearchItems]
  );

  // 处理列选择变化
  const handleColumnSelectionChange = useCallback(
    (
      _selectedDataIndexes: string[],
      dataSource: Array<{
        dataIndex: string;
        hidden: boolean;
        width?: number;
        ellipsis?: boolean;
        sort?: number;
      }>
    ) => {
      if (!enabled) return;

      const columnsConfig: ColumnCacheData[] = dataSource?.map((item) => ({
        dataIndex: item.dataIndex,
        hidden: item.hidden,
        width: item.width,
        ellipsis: item.ellipsis,
        sort: item.sort,
      }));
      const cachedData: CacheData = wsCache.get(cacheKey) || {};
      wsCache.set(cacheKey, {
        ...cachedData,
        columnsConfig,
      });

      // 同步更新 columns 状态和缓存引用
      setColumnsState((prevColumns) => {
        const updatedColumns = prevColumns?.map((col) => {
          const dataSourceItem = dataSource.find(
            (item) => item.dataIndex === col.dataIndex
          );
          if (dataSourceItem) {
            return {
              ...col,
              hidden: dataSourceItem.hidden,
              width:
                dataSourceItem.width !== undefined
                  ? dataSourceItem.width
                  : col.width,
              ellipsis:
                dataSourceItem.ellipsis !== undefined
                  ? dataSourceItem.ellipsis
                  : col.ellipsis,
              sort:
                dataSourceItem.sort !== undefined
                  ? dataSourceItem.sort
                  : col.sort,
            };
          }
          return col;
        });

        // 按 sort 排序
        updatedColumns.sort((a, b) => (a.sort ?? 0) - (b.sort ?? 0));

        // 更新缓存引用
        const currentColumnsKey = JSON.stringify(columnsConfig);
        prevColumnsRef.current = currentColumnsKey;

        return updatedColumns;
      });
    },
    [cacheKey, enabled]
  );

  // 当 columns 变化时自动保存到缓存
  useEffect(() => {
    if (enabled && columns.length > 0) {
      const columnsConfig: ColumnCacheData[] = columns
        .filter((col) => col.dataIndex) // 过滤掉没有 dataIndex 的列
        ?.map((col) => ({
          dataIndex: col.dataIndex!,
          hidden: col.hidden || false,
          width: typeof col.width === "number" ? col.width : undefined,
          ellipsis:
            typeof col.ellipsis === "boolean" ? col.ellipsis : undefined,
          sort: typeof col.sort === "number" ? col.sort : undefined,
        }));
      const currentColumnsKey = JSON.stringify(columnsConfig);

      // 只有当配置真正改变时才更新缓存
      if (currentColumnsKey !== prevColumnsRef.current) {
        const cachedData: CacheData = wsCache.get(cacheKey) || {};
        wsCache.set(cacheKey, {
          ...cachedData,
          columnsConfig,
        });
        prevColumnsRef.current = currentColumnsKey;
      }
    }
  }, [columns, cacheKey, enabled]);

  return {
    searchItems,
    columns,
    setSearchItems,
    setColumns,
    handleSearchFilterChange,
    handleColumnSelectionChange,
  };
}
