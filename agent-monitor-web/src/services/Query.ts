import { isNull } from "lodash";

/**
 *@property EQ(" = ", "等于"),
 *@property NE(" <> ", "不等于"),
 *@property LK(" like ", "包含"),
 *@property LLK(" like ", "左包含"),
 *@property RLK(" like ", "右包含"),
 *@property NLK(" not like ", "不包含"),
 *@property IN(" in ", "在..中"),
 *@property NIN(" not in ", "不在..中"),
 *@property LT(" < ", "小于"),
 *@property LE(" <= ", "小于等于"),
 *@property GT(" > ", "大于"),
 *@property GE(" >= ", "大于等于"),
 *@property BT(" between ", "位于..和..之间"),
 *@property IS(" is ", "是"),
 *@property NIS(" is not ", "不是"),
 *@property ASC(" asc ", "升序"),
 *@property DESC(" desc ", "降序"),
 *@property AND("and", "and"),
 *@property OR("or", "or"),
 */
export type ModeType =
  | "EQ"
  | "NE"
  | "LK"
  | "LLK"
  | "RLK"
  | "NLK"
  | "IN"
  | "NIN"
  | "LT"
  | "LE"
  | "GT"
  | "GE"
  | "BT"
  | "IS"
  | "NIS"
  | "ASC"
  | "DESC"
  | "AND"
  | "OR"
  | string;

export type OrderType = "desc" | "asc";

export type RuleNameType = string;

export type LogicType = "and" | "or" | "not" | string;

/**
 * 过滤条件
 * @property {sting} k 查询键
 * @property {unknown} v 查询值
 * @property {ModeType} m 查询方式 默认值（LK）非必填
 * @property {LogicType} t1 条件间逻辑 默认值（and）非必填
 * @property {number} s 序列 默认值（0）非必填
 * @property {RuleNameType} n 规则名称 默认值（a1）非必填
 * @property {LogicType} t 规则间逻辑 默认值（and）非必填
 * @property {Omit<filterType, 'w' | "t1">[]} w 过滤条件 默认值（[]）非必填
 */
export interface filterType {
  k: string;
  v: unknown;
  m?: ModeType;
  t1?: LogicType;
  s?: number;
  n?: RuleNameType;
  t?: LogicType;
  w?: Omit<filterType, "w" | "t1">[];
}

export default class Query {
  get value() {
    return this.toEncode();
  }

  get valueObj() {
    return this.toQuery();
  }

  /**
   * w 过滤条件
   *
   * @see {Query#r}
   * @deprecated
   */
  private _w: filterType[] = [];

  /**
   * r 过滤条件
   */
  private _r: Partial<Omit<filterType, "t1">>[] = [];

  /**
   * o 排序
   */
  private _o: { k: string; t?: OrderType }[] = [];

  /**
   * j 左连接、右连接 ON 后面过滤条件，主要面向后端开发人员
   */
  private _j: filterType[] = [];

  /**
   * p 分页条件
   */
  private _p: { n?: number; s?: number; c?: 1 | 0 } | undefined = {
    n: 1,
    s: 10,
  };

  /**
   * s 过滤字段
   */
  private _s: { v?: string | string[] } = {};

  /**
   * 过滤条件
   * @param objKv 表单数据
   * @param objKm 数据查询方式
   * @param objRn 规则名称
   * @returns
   */
  public toF = <T = Record<string, unknown>>(
    objKv: Partial<T>,
    objKm: { [K in keyof T]?: ModeType },
    objRn: { [K in keyof T]?: RuleNameType },
    objKt?: { [K in keyof T]?: LogicType }
  ) => {
    this._r = [];

    for (const key in objKv) {
      this.toR(
        key,
        objKv[key],
        objKm[key],
        objKt?.[key] || "and",
        0,
        objRn[key]
      );
    }
  };

  /**
   * 过滤条件
   * @param {sting} k 查询键
   * @param {unknown} v 查询值
   * @param {ModeType} m 查询方式 默认值（LK）非必填
   * @param {LogicType} t1 条件间逻辑 默认值（and）非必填
   * @param {number} s 序列 默认值（0）非必填
   * @param {RuleNameType} n 规则名称 默认值（a1）非必填
   * @param {LogicType} t 规则间逻辑 默认值（and）非必填
   * @param {boolean} w 允许查询键重复 默认值（false）非必填
   * @returns
   */
  public toR = (
    k: string,
    v: unknown,
    m: ModeType = "LK",
    t1: LogicType = "and",
    s: number = 0,
    n: RuleNameType = "a1",
    t: LogicType = "and",
    w: boolean = false
  ) => {
    const passType =
      typeof v !== "number" &&
      typeof v !== "bigint" &&
      typeof v !== "boolean" &&
      !isNull(v);

    if (
      (passType && !v) ||
      (m === "IN" && Array.isArray(v) && v.length === 0) ||
      !m
    ) {
      return;
    }

    const rObj = this._r.find((x) => x.n === n);
    if (rObj && (!rObj.w?.find((x) => x.k === k) || w)) {
      rObj.w?.push({
        k: k,
        v: v,
        m: m,
        t: t1,
        s: s,
      });
    } else {
      if (!this._r?.find((x) => x.n === n)) {
        this._r?.push({
          n: n,
          t: t,
          w: [
            {
              k: k,
              v: v,
              m: m,
              t: t1,
              s: s,
            },
          ],
        });
      }
    }
  };

  /**
   * 等于查询
   * @param {sting} k 查询键
   * @param {unknown} v 查询值
   * @param {LogicType} t1 条件间逻辑 默认值（and）非必填
   * @param {number} s 序列 默认值（0）非必填
   * @param {RuleNameType} n 规则名称 默认值（a1）非必填
   * @param {LogicType} t 规则间逻辑 默认值（and）非必填
   */
  public toEqual = (
    k: string,
    v: unknown,
    t1: LogicType = "and",
    s: number = 0,
    n: RuleNameType = "a1",
    t: LogicType = "and"
  ) => {
    this._r = [];

    this.toR(k, v, "EQ", t1, s, n, t);
  };

  /**
   * 包含查询
   * @param {sting} k 查询键
   * @param {unknown} v 查询值
   * @param {LogicType} t1 条件间逻辑 默认值（and）非必填
   * @param {number} s 序列 默认值（0）非必填
   * @param {RuleNameType} n 规则名称 默认值（a1）非必填
   * @param {LogicType} t 规则间逻辑 默认值（and）非必填
   */
  public toLike = (
    k: string,
    v: unknown,
    t1: LogicType = "and",
    s: number = 0,
    n: RuleNameType = "a1",
    t: LogicType = "and"
  ) => {
    this._r = [];

    this.toR(k, v, "LK", t1, s, n, t);
  };

  /**
   * 排序
   * @param {key} k 排序字段
   * @param {OrderType} t 升序还是降序
   */
  public toO = (k: string, t: OrderType = "desc") => {
    this._o = [];

    if (typeof k === "undefined" || k === "") {
      return;
    }
    if (this._o.find((x) => x.k === k)) {
      this._o.find((x) => x.k === k)!.t = t;
    } else {
      this._o.push({
        k: k,
        t: t,
      });
    }
  };
  public toOArr = (o: { k: string; t: OrderType }[]) => {
    this._o = o;
  };

  /**
   * 分页条件
   * @param {number} n 第几页
   * @param {number} s 每页条数
   * @param {1 | 0} c 是否查询总数，非必填，如果设置为1，则不会查询总数
   */
  public toP = (n: number, s: number, c: 1 | 0 = 0) => {
    this._p = {
      n: n,
      s: s,
      c: c,
    };
  };

  /**
   * 过滤字段
   * @param {string} v 过滤字段，支持String类型多个字段(",")逗号隔开，如："username,mob"，或者进行数组赋值，如：["username", "mob"]
   */
  public toS = (v: string) => {
    this._s = {
      v: v,
    };
  };

  /**
   * query对象
   */
  public toQuery = () => {
    return {
      w: this._w,
      r: this._r,
      o: this._o,
      j: this._j,
      p: this._p,
      s: this._s,
    };
  };

  /**
   * query对象转string
   */
  public toJsonStr = () => {
    return JSON.stringify({
      w: this._w,
      r: this._r,
      o: this._o,
      j: this._j,
      p: this._p,
      s: this._s,
    });
  };

  /**
   * 查询条件url编码
   */
  public toEncode = () => {
    return encodeURIComponent(this.toJsonStr());
  };

  /**
   * 清空查询条件
   */
  public clear = () => {
    this._w = [];
    this._r = [];
    this._o = [];
    this._j = [];
    this._p = undefined;
    this._s = {};
  };
}
