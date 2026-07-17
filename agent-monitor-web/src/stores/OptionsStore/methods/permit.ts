import { getEnum, getRoleList, getUserList } from "@/services/apis/enum";
import { SelectOption } from "@hsu-react/ui";
import { makeAutoObservable } from "mobx";

/**
 * 权限相关的选项方法
 */
export default class PermitMethods {
  // 角色
  private _role: SelectOption[] = [];
  // 角色类型
  private _roleType: SelectOption[] = [];
  // 用户
  private _user: SelectOption[] = [];
  // 是否脱敏
  private _maskStatus: SelectOption[] = [];

  constructor() {
    makeAutoObservable(this);
  }

  // 角色
  public getRole = () => {
    getRoleList().then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._role = list?.map((item) => ({
          label: item.nm,
          value: item.id,
        }));
      }
    });
  };

  // 角色类型
  public getRoleType = () => {
    getEnum("SysUserEn$Type").then((res) => {
      if (res.code === 0) {
        const { list = [] } = res.data;
        this._roleType = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 用户
  public getUser = () => {
    getUserList().then((res) => {
      if (res.code === 0) {
        const { list = [] } = res.data;
        this._user = list?.map((item) => ({
          label: item.nickname,
          value: item.id,
        }));
      }
    });
  };

  // 是否脱敏
  public getMaskStatus = () => {
    getEnum("SysRoleDataLevelScopeEn$MaskStatus").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._maskStatus = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // Getters
  get role() {
    return this._role;
  }
  get roleType() {
    return this._roleType;
  }
  get user() {
    return this._user;
  }
  get maskStatus() {
    return this._maskStatus;
  }
}
