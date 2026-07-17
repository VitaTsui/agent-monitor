import {
  getEnum,
  getSmsConfigList,
  getSmsTemplateList,
} from "@/services/apis/enum";
import { SelectOption } from "@hsu-react/ui";
import { makeAutoObservable } from "mobx";

/**
 * 系统管理相关的选项方法
 */
export default class SysmgmtMethods {
  // 菜单分类
  private _menuCat: SelectOption[] = [];
  // 菜单类型
  private _menuType: SelectOption[] = [];
  // 文件配置状态
  private _fileConfigStatus: SelectOption[] = [];
  // 文件配置类型
  private _fileConfigType: SelectOption[] = [];
  // 文件配置环境
  private _fileConfigActive: SelectOption[] = [];
  // 短信配置环境
  private _smsConfigActive: SelectOption[] = [];
  // 短信配置类型
  private _smsConfigType: SelectOption[] = [];
  // 短信配置状态
  private _smsConfigStatus: SelectOption[] = [];
  // 短信模板类型
  private _smsTemplateType: SelectOption[] = [];
  // 短信日志状态
  private _smsLogStatus: SelectOption[] = [];
  // 短信配置列表
  private _smsConfigList: SelectOption[] = [];
  // 短信模板列表
  private _smsTemplateList: SelectOption[] = [];

  constructor() {
    makeAutoObservable(this);
  }

  // 菜单分类
  public getMenuCat = () => {
    getEnum("SysRscoEn$Cat").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._menuCat = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 菜单类型
  public getMenuType = () => {
    getEnum("SysRscoEn$Type").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._menuType = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 文件配置状态
  public getFileConfigStatus = () => {
    getEnum("SysFileConfEn$Status").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._fileConfigStatus = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 文件配置类型
  public getFileConfigType = () => {
    getEnum("SysFileConfEn$Type").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._fileConfigType = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 文件配置环境
  public getFileConfigActive = () => {
    getEnum("SysFileConfEn$Active").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._fileConfigActive = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 短信配置环境
  public getSmsConfigActive = () => {
    getEnum("SysSmsConfEn$Active").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._smsConfigActive = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 短信配置类型
  public getSmsConfigType = () => {
    getEnum("SysSmsConfEn$Type").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._smsConfigType = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 短信配置状态
  public getSmsConfigStatus = () => {
    getEnum("SysSmsConfEn$Status").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._smsConfigStatus = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 短信模板类型
  public getSmsTemplateType = () => {
    getEnum("SysSmsTemplateEn$Type").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._smsTemplateType = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 短信日志状态
  public getSmsLogStatus = () => {
    getEnum("SysSmsLogEn$Status").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._smsLogStatus = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // 短信配置列表
  public getSmsConfigList = () => {
    getSmsConfigList().then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._smsConfigList = list?.map((item) => ({
          ...item,
          label: item.nm,
          value: item.id,
        }));
      }
    });
  };

  // 短信模板列表
  public getSmsTemplateList = () => {
    getSmsTemplateList().then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._smsTemplateList = list?.map((item) => ({
          ...item,
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // Getters
  get menuCat() {
    return this._menuCat;
  }
  get menuType() {
    return this._menuType;
  }
  get fileConfigStatus() {
    return this._fileConfigStatus;
  }
  get fileConfigType() {
    return this._fileConfigType;
  }
  get fileConfigActive() {
    return this._fileConfigActive;
  }
  get smsConfigActive() {
    return this._smsConfigActive;
  }
  get smsConfigType() {
    return this._smsConfigType;
  }
  get smsConfigStatus() {
    return this._smsConfigStatus;
  }
  get smsTemplateType() {
    return this._smsTemplateType;
  }
  get smsLogStatus() {
    return this._smsLogStatus;
  }
  get smsConfigList() {
    return this._smsConfigList;
  }
  get smsTemplateList() {
    return this._smsTemplateList;
  }
}
