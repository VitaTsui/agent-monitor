import { SelectOption } from "@hsu-react/ui";

export const YES_NO: SelectOption[] = [
  { label: "是", value: 1 },
  { label: "否", value: 0 },
];

export const USER_STATUS: SelectOption[] = [
  { label: "正常", value: 0 },
  { label: "冻结", value: 1 },
];

export const MENU_SHOW: SelectOption[] = [
  { label: "显示", value: 0 },
  { label: "隐藏", value: 1 },
];

export const APILOG_STATUS: SelectOption[] = [
  { label: "成功", value: 0 },
  { label: "失败", value: 1 },
];

export const BASE_FUNCTION_TYPE: SelectOption[] = [
  { label: "查看", value: "page,info,list,query" },
  { label: "新增", value: "add" },
  { label: "修改", value: "upd" },
  { label: "删除", value: "del" },
  { label: "导出", value: "expt" },
  { label: "导入", value: "impt" },
  { label: "审核", value: "audit" },
];

export const API_METHOD: SelectOption[] = [
  { label: "GET", value: "GET" },
  { label: "POST", value: "POST" },
  { label: "PUT", value: "PUT" },
  { label: "DELETE", value: "DELETE" },
];

export const LIST_OPERATION_CONDITION: SelectOption[] = [
  { label: "=", value: "=" },
  { label: "!=", value: "!=" },
  { label: ">", value: ">" },
  { label: ">=", value: ">=" },
  { label: "<", value: "<" },
  { label: "<=", value: "<=" },
  { label: "LIKE", value: "LIKE" },
  { label: "BETWEEN", value: "BETWEEN" },
];

export const HTML_TYPE: SelectOption[] = [
  { label: "文本框", value: "input" },
  { label: "文本域", value: "textarea" },
  { label: "下拉框", value: "select" },
  { label: "单选框", value: "radio" },
  { label: "复选框", value: "checkbox" },
  { label: "日期控件", value: "datetime" },
  { label: "图片上传", value: "imageUpload" },
  { label: "文件上传", value: "fileUpload" },
  { label: "富文本控件", value: "editor" },
];

export const JAVA_TYPE: SelectOption[] = [
  { label: "Long", value: "Long" },
  { label: "String", value: "String" },
  { label: "Integer", value: "Integer" },
  { label: "Double", value: "Double" },
  { label: "BigDecimal", value: "BigDecimal" },
  { label: "LocalDateTime", value: "LocalDateTime" },
  { label: "Boolean", value: "Boolean" },
];

export const DICT_TYPE: SelectOption[] = [
  { label: "非字典", value: 0 },
  { label: "枚举类", value: 1 },
  { label: "自定义输入", value: 2 },
];

export const CODETEMP_STATUS: SelectOption<boolean>[] = [
  { label: "启用", value: true },
  { label: "禁用", value: false },
];
