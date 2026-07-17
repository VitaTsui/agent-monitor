import { FormItemProps } from "@hsu-react/ui";
import { get_string_size } from "hsu-utils";
import { useMemo } from "react";

interface Font {
  style?: string;
  variant?: string;
  weight?: string;
  size?: number;
  lineHeight?: number;
  family?: string;
}

const getLabelWidth = (
  formItem: FormItemProps[],
  font: Font = { size: 14 },
  min?: boolean | number
) => {
  let _labelWitdh: number = 0;

  if (formItem) {
    _labelWitdh = formItem.reduce(
      (prev, curr) => {
        let _labelWidth: number = prev;

        const _label = curr.label;
        if (typeof _label === "string" && _label) {
          const { width } = get_string_size(_label, font);
          _labelWidth = width;

          if (
            curr.colon !== false &&
            ((curr.required === true && curr.requiredPosition !== "after") ||
              curr.required !== true)
          ) {
            const { width } = get_string_size(":", font);
            _labelWidth += width + 10;
          }

          if (curr.required && curr.hideRequired !== true) {
            const { width } = get_string_size("*", font);
            _labelWidth += width + 4;
          }

          if (
            curr.tips &&
            (!curr.tips?.icon || typeof curr.tips?.icon === "string")
          ) {
            _labelWidth += 22;
          }

          if (curr.layout === "vertical") {
            _labelWidth = 0;
          }
        }

        if (!!min && prev !== 0) {
          if (typeof min === "number") {
            return min;
          }

          return _labelWidth < prev ? _labelWidth : prev;
        }

        return _labelWidth > prev ? _labelWidth : prev;
      },
      typeof min === "number" ? min : 0
    );
  }

  return _labelWitdh;
};

export default function useLabelWidth(
  formItem: FormItemProps[] = [],
  font: Font = {
    size: 14,
    family:
      "-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,'Helvetica Neue',Arial,'Noto Sans',sans-serif,'Apple Color Emoji','Segoe UI Emoji','Segoe UI Symbol','Noto Color Emoji'",
  },
  min?: boolean | number
): [
  number,
  (formItem: FormItemProps[], font?: Font, min?: boolean | number) => number
] {
  const labelWidth = useMemo(
    () => getLabelWidth(formItem, font, min),
    [formItem, font, min]
  );

  return [labelWidth, getLabelWidth];
}
