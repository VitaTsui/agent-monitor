import { makeAutoObservable } from "mobx";
import { SelectOption } from "@hsu-react/ui";

// 导入各个功能模块的方法类
import PermitMethods from "./methods/permit";
import SysmgmtMethods from "./methods/sysmgmt";
import SyslogMethods from "./methods/syslog";

/**
 * @param MENU_CAT 菜单分类
 * @param MENU_TYPE 菜单类型
 * @param ROLE 角色
 * @param ROLE_TYPE 角色类型
 * @param USER 用户
 * @param MASK_STATUS 是否脱敏
 * @param FILE_CONFIG_STATUS 文件配置状态
 * @param FILE_CONFIG_TYPE 文件配置类型
 * @param FILE_CONFIG_ACTIVE 文件配置环境
 * @param SMS_CONFIG_ACTIVE 短信配置环境
 * @param SMS_CONFIG_TYPE 短信配置类型
 * @param SMS_CONFIG_STATUS 短信配置状态
 * @param SMS_TEMPLATE_TYPE 短信模板类型
 * @param SMS_LOG_STATUS 短信日志状态
 * @param SMS_CONFIG_LIST 短信配置列表
 * @param SMS_TEMPLATE_LIST 短信模板列表
 * @param LOGIN_LOG_STATUS 登录日志状态
 * @param JOB_LOG_STATUS 任务日志状态
 * @param LARGE_MODEL_API_KEY_LIST 大模型API_KEY列表（占位，模板内未接入真实大模型，需自行扩展）
 */
export type OptionsType =
  | "MENU_CAT"
  | "MENU_TYPE"
  | "ROLE"
  | "ROLE_TYPE"
  | "USER"
  | "MASK_STATUS"
  | "FILE_CONFIG_STATUS"
  | "FILE_CONFIG_TYPE"
  | "FILE_CONFIG_ACTIVE"
  | "SMS_CONFIG_ACTIVE"
  | "SMS_CONFIG_TYPE"
  | "SMS_CONFIG_STATUS"
  | "SMS_TEMPLATE_TYPE"
  | "SMS_LOG_STATUS"
  | "SMS_CONFIG_LIST"
  | "SMS_TEMPLATE_LIST"
  | "LOGIN_LOG_STATUS"
  | "JOB_LOG_STATUS"
  | "LARGE_MODEL_API_KEY_LIST";

class CommonOptions {
  private permitMethods: PermitMethods;
  private sysmgmtMethods: SysmgmtMethods;
  private syslogMethods: SyslogMethods;

  constructor() {
    this.permitMethods = new PermitMethods();
    this.sysmgmtMethods = new SysmgmtMethods();
    this.syslogMethods = new SyslogMethods();

    makeAutoObservable(this);
  }

  // 系统管理相关方法 - 委托给 SysmgmtMethods
  public getMenuCat = () => this.sysmgmtMethods.getMenuCat();
  public getMenuType = () => this.sysmgmtMethods.getMenuType();
  public getFileConfigStatus = () => this.sysmgmtMethods.getFileConfigStatus();
  public getFileConfigType = () => this.sysmgmtMethods.getFileConfigType();
  public getFileConfigActive = () => this.sysmgmtMethods.getFileConfigActive();
  public getSmsConfigActive = () => this.sysmgmtMethods.getSmsConfigActive();
  public getSmsConfigType = () => this.sysmgmtMethods.getSmsConfigType();
  public getSmsConfigStatus = () => this.sysmgmtMethods.getSmsConfigStatus();
  public getSmsTemplateType = () => this.sysmgmtMethods.getSmsTemplateType();
  public getSmsLogStatus = () => this.sysmgmtMethods.getSmsLogStatus();
  public getSmsConfigList = () => this.sysmgmtMethods.getSmsConfigList();
  public getSmsTemplateList = () => this.sysmgmtMethods.getSmsTemplateList();

  // 权限相关方法 - 委托给 PermitMethods
  public getRole = () => this.permitMethods.getRole();
  public getRoleType = () => this.permitMethods.getRoleType();
  public getUser = () => this.permitMethods.getUser();
  public getMaskStatus = () => this.permitMethods.getMaskStatus();

  // 系统日志相关方法 - 委托给 SyslogMethods
  public getLoginLogStatus = () => this.syslogMethods.getLoginLogStatus();
  public getJobLogStatus = () => this.syslogMethods.getJobLogStatus();

  // 大模型 API_KEY 列表（占位实现：模板未接入真实大模型，syslog 下 ChatModal 引用此项时不会发起请求）
  public getLargeModelApiKeyList = () => undefined;

  public options = (optionsType: OptionsType): SelectOption[] => {
    switch (optionsType) {
      case "MENU_CAT":
        return this.sysmgmtMethods.menuCat;
      case "MENU_TYPE":
        return this.sysmgmtMethods.menuType;
      case "ROLE":
        return this.permitMethods.role;
      case "ROLE_TYPE":
        return this.permitMethods.roleType;
      case "USER":
        return this.permitMethods.user;
      case "MASK_STATUS":
        return this.permitMethods.maskStatus;
      case "FILE_CONFIG_STATUS":
        return this.sysmgmtMethods.fileConfigStatus;
      case "FILE_CONFIG_TYPE":
        return this.sysmgmtMethods.fileConfigType;
      case "FILE_CONFIG_ACTIVE":
        return this.sysmgmtMethods.fileConfigActive;
      case "SMS_CONFIG_ACTIVE":
        return this.sysmgmtMethods.smsConfigActive;
      case "SMS_CONFIG_TYPE":
        return this.sysmgmtMethods.smsConfigType;
      case "SMS_CONFIG_STATUS":
        return this.sysmgmtMethods.smsConfigStatus;
      case "SMS_TEMPLATE_TYPE":
        return this.sysmgmtMethods.smsTemplateType;
      case "SMS_LOG_STATUS":
        return this.sysmgmtMethods.smsLogStatus;
      case "SMS_CONFIG_LIST":
        return this.sysmgmtMethods.smsConfigList;
      case "SMS_TEMPLATE_LIST":
        return this.sysmgmtMethods.smsTemplateList;
      case "LOGIN_LOG_STATUS":
        return this.syslogMethods.loginLogStatus;
      case "JOB_LOG_STATUS":
        return this.syslogMethods.jobLogStatus;
      case "LARGE_MODEL_API_KEY_LIST":
        return [];
      default:
        return [];
    }
  };
}

const OptionsStore = new CommonOptions();

export const Options = OptionsStore.options;

export default OptionsStore;
