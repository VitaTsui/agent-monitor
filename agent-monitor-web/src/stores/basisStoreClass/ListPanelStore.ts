import { computed, makeObservable, observable } from "mobx";

import Query, {
  LogicType,
  ModeType,
  OrderType,
  RuleNameType,
} from "@/services/Query";
import FormModalStore from "./FormModalStore";
import { ListRes } from "@/services/ResType";
import { ResType } from "@/services/Axios";
import { message, notification } from "antd";
import wsCache, { CACHE_KEY } from "@/utils/wsCache";
import { Equal } from "hsu-utils";

export type searchModeType<T> = { [K in keyof T]?: ModeType };
export type searchRNType<T> = { [K in keyof T]?: RuleNameType };
export type searchKTType<T> = { [K in keyof T]?: LogicType };

/**
 * S: 查询条件类型
 * D：列表数据类型
 */
export default class ListPanelStore<
  S = Record<string, unknown>,
  D = Record<string, unknown>
> extends FormModalStore<D> {
  // 请求
  protected accessor _query: Query = new Query();

  constructor() {
    super();
    makeObservable(this);
  }

  /**
   * 正在加载
   */
  @computed
  get isLoading() {
    return this._isLoading;
  }
  @observable
  protected accessor _isLoading: boolean = true;

  /**
   * 排序
   */
  @computed
  get order() {
    return this._order;
  }
  @observable
  protected accessor _order: { k: string; t: OrderType } | undefined = undefined;
  protected accessor _initOrder: { k: string; t: OrderType } = {
    k: "crtTm",
    t: "desc",
  };
  public onOrderChange = (order?: { k: string; t: OrderType }) => {
    this._order =
      order ||
      (Equal.ObjEqual(this._initOrder, this._order) && !!this._order
        ? undefined
        : this._initOrder);

    this._query.toOArr(this._order ? [this._order] : []);

    this._isLoading = true;
    // 请求前置空旧列表，避免新数据返回前展示过期行
    this._dataSource = [];

    this.getDataSource();
  };

  /**
   * 查询
   */
  @computed
  get searchData() {
    return this._searchData;
  }
  @observable
  protected accessor _searchData: Partial<S> = {};
  protected accessor _initSearchData: Partial<S> = {};
  protected accessor _staticSearchData: Partial<S> = {};
  protected accessor _modeType: Partial<Record<keyof Partial<S>, ModeType>> =
    {};
  protected accessor _ruleName: Partial<
    Record<keyof Partial<S>, RuleNameType>
  > = {};
  protected accessor _logicType: Partial<Record<keyof Partial<S>, LogicType>> =
    {};
  public setSearchData = (searchData: Partial<S> = {}) => {
    this._searchData = {
      ...this._initSearchData,
      ...searchData,
      ...this._staticSearchData,
    };

    this._query.toF(
      this._searchData,
      this._modeType as Partial<Record<keyof S, ModeType>>,
      this._ruleName,
      this._logicType
    );

    // 条数仅在查询时获取，翻页/排序不重复请求
    this.getTotal();

    this.changePage({ num: 1 });
  };
  public initSearchData = (searchData?: Partial<S>) => {
    if (searchData) {
      this._initSearchData = searchData;
    }

    if (!this._order) {
      this._order = this._initOrder;
    }
    if (this._order) {
      this._query.toOArr([this._order]);
    }

    setTimeout(() => {
      this.setSearchData(this._initSearchData);
    }, 500);
  };

  /**
   * 分页
   */
  @computed
  get page() {
    return this._page;
  }
  @observable
  protected accessor _page: { num: number; size: number } = {
    num: 1,
    size: wsCache.get(CACHE_KEY.PAGE_SIZE)?.[window.location.pathname] || 20,
  };
  protected accessor _c: 1 | 0 = 0;
  public changePage = (
    page: { num?: number; size?: number },
    search: boolean = true
  ) => {
    if (!search) {
      this._page = { ...this._page, ...page };

      return;
    }

    this._isLoading = true;
    // 请求前置空旧列表，避免新数据返回前展示过期行
    this._dataSource = [];

    this._page = { ...this._page, ...page };

    wsCache.set(CACHE_KEY.PAGE_SIZE, {
      ...(wsCache.get(CACHE_KEY.PAGE_SIZE) || {}),
      [window.location.pathname]: this._page.size,
    });

    this._query.toP(this._page.num, this._page.size, this._c);

    this.getDataSource();
  };
  public resetPage = () => {
    this.changePage({ num: 1 });
  };
  public onShowSizeChange = (page: { num?: number; size?: number }) => {
    this._page = { ...this._page, ...page };
  };

  /**
   * 列表
   */
  @computed
  get total() {
    return this._total;
  }
  @observable
  protected accessor _total: number = 0;

  /**
   * 条数加载中
   * 独立条数请求（getTotal）进行时为 true，由有独立条数请求的子类维护
   */
  @computed
  get totalLoading() {
    return this._totalLoading;
  }
  @observable
  protected accessor _totalLoading: boolean = false;
  @computed
  get dataSource(): Array<D> {
    return this._dataSource;
  }
  @observable
  protected accessor _dataSource: Array<D> = [];
  public getDataSource = () => {
    // 由子类实现具体的数据获取逻辑
  };

  /**
   * 局部更新单行数据（按主键就地合并），不触发整表刷新
   * @param id 主键值
   * @param row 要合并的字段
   * @param key 主键字段名，默认 "id"
   */
  protected _patchRow = (
    id: number | string,
    row: Partial<D>,
    key: string = "id"
  ) => {
    const index = this._dataSource.findIndex(
      (item) => String((item as Record<string, unknown>)[key]) === String(id)
    );

    if (index > -1) {
      this._dataSource[index] = { ...this._dataSource[index], ...row };
    }
  };

  // 单行刷新请求序号，用于丢弃乱序返回的过期响应
  private accessor _rowRefreshSeq: Map<string, number> = new Map();

  /**
   * 按主键重新拉取单行数据并就地合并，不触发整表刷新
   * 走列表接口保证行结构与列表一致；未查到该行时回退整表刷新
   * @param id 主键
   * @param fetchList 列表接口（与 getDataSource 用同一个）
   * @param key 主键字段名，默认 "id"
   */
  protected _refreshRowData = (
    id: number | string,
    fetchList: (params: { query: string }) => Promise<ResType<ListRes<D>>>,
    key: string = "id"
  ) => {
    const seq = (this._rowRefreshSeq.get(String(id)) ?? 0) + 1;
    this._rowRefreshSeq.set(String(id), seq);

    const query = new Query();
    query.toEqual(key, id);
    query.toP(1, 1, 1);

    fetchList({ query: query.value })
      .then((res) => {
        if (this._rowRefreshSeq.get(String(id)) !== seq) {
          return;
        }

        const row = res.code === 0 ? res.data.list?.[0] : undefined;

        if (row) {
          this._patchRow(id, row, key);
        } else {
          this.getDataSource();
        }
      })
      .catch(() => {});
  };

  /**
   * 获取条数
   * 仅在查询（setSearchData）时调用，翻页/排序不触发
   * 由有独立条数请求的子类实现；无独立条数请求的列表无需重写
   */
  public getTotal = () => {
    // 由子类实现具体的条数获取逻辑
  };

  /**
   * 删除
   * @param id
   */
  public delData = (_id: number | string) => {
    // 由子类实现具体的删除逻辑
  };

  /**
   * 导入文件
   * @param file
   */
  public uploadList = (_file: FormData) => {
    // 由子类实现具体的导入逻辑
  };

  /**
   * 消息处理
   * @param res
   */
  protected _message = (res?: ResType) => {
    if (res?.code === 0) {
      if (typeof res?.data === "string") {
        notification.success({
          message: res.data,
        });
      } else {
        message.success(res?.msg ?? "成功");
      }
    } else {
      notification.error({
        message: res?.msg ?? "失败",
      });
    }
  };

  /**
   * 重置Store
   */
  public resetStore = () => {
    this._searchData = {};
    this._order = undefined;
    this._page = {
      num: 1,
      size: wsCache.get(CACHE_KEY.PAGE_SIZE)?.[window.location.pathname] || 20,
    };
    this._dataSource = [];
    this._total = 0;
    this._totalLoading = false;
    this._isLoading = true;
    this._rowRefreshSeq.clear();
    this._query.clear();

    this.resetFormData();

    this._resetStore();
  };
  protected _resetStore = () => {};
}
