import "dayjs/locale/zh-cn";

import React, { ReactNode, useEffect } from "react";

import { ConfigProvider } from "antd";
import { IntlProvider } from "react-intl";
import I18nStore from "./I18nStore";
import { observer } from "mobx-react-lite";

interface InternationalizationProps {
  children: ReactNode;
}

const Internationalization: React.FC<InternationalizationProps> = observer(
  ({ children }) => {
    const { messages, locale, antLocale } = I18nStore;

    useEffect(() => {
      const html = document.documentElement;
      html.classList.remove("lang-en-US", "lang-zh-CN");
      html.classList.add(`lang-${locale}`);
    }, [locale]);

    return (
      <IntlProvider locale={locale} messages={messages}>
        <ConfigProvider locale={antLocale}>{children}</ConfigProvider>
      </IntlProvider>
    );
  }
);

export default Internationalization;
