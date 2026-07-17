import { SelectOption } from "@hsu-react/ui";
import { getEnum } from "@/services/apis/enum";
import { makeAutoObservable } from "mobx";

/**
 * 系统日志相关的选项方法
 */
export default class SyslogMethods {
  // 登录日志状态
  private _loginLogStatus: SelectOption[] = [];
  // 任务日志状态
  private _jobLogStatus: SelectOption[] = [];

  constructor() {
    makeAutoObservable(this);
  }

  // 登录日志状态
  public getLoginLogStatus = () => {
    getEnum("SysLogLoginEn$Status").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._loginLogStatus = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 任务日志状态
  public getJobLogStatus = () => {
    getEnum("SysJobLogEn$Status").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._jobLogStatus = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // Getters
  get loginLogStatus() {
    return this._loginLogStatus;
  }
  get jobLogStatus() {
    return this._jobLogStatus;
  }
}
