import { IntlFormatters, createIntl, createIntlCache } from "react-intl";

import { Locale } from "antd/lib/locale";
import enUS from "antd/locale/en_US";
import en_US from "./locales/en-US";
import { makeAutoObservable } from "mobx";
import zhCN from "antd/locale/zh_CN";
import zh_CN from "./locales/zh-CN";
import wsCache, { CACHE_KEY } from "@/utils/wsCache";

type MessagesType = Record<string, string>;

type FormatMessage = IntlFormatters["formatMessage"];

export interface LocaleOption {
  label: string;
  value: string;
}
export const LocaleOptions: LocaleOption[] = [
  { label: "中文简体", value: "zh-CN" },
  { label: "English", value: "en-US" },
];

class I18n {
  get locale() {
    return this._locale;
  }
  private _locale: string = "zh-CN";

  get messages() {
    return this._messages;
  }
  private _messages: MessagesType = zh_CN;

  get antLocale() {
    return this._antLocale;
  }
  private _antLocale: Locale = zhCN;

  get formatMessage() {
    return this._formatMessage;
  }
  private _formatMessage!: FormatMessage;

  constructor() {
    makeAutoObservable(this);
    this.setLocale(wsCache.get("lang") || Config.locale || "zh-CN");
  }

  public setLocale = (locale: string = "zh-CN") => {
    this._locale = locale;

    wsCache.set(CACHE_KEY.LANG, locale);

    switch (locale) {
      case "en-US":
        this._antLocale = enUS;
        this._messages = en_US;
        break;
      default:
        this._antLocale = zhCN;
        this._messages = zh_CN;
        break;
    }

    this._setFormatMessage();
  };

  private _setFormatMessage = () => {
    const cache = createIntlCache();

    const intl = createIntl(
      {
        locale: this._locale,
        messages: this._messages,
      },
      cache
    );

    this._formatMessage = intl.formatMessage;
  };
}

const I18nStore = new I18n();
export default I18nStore;
