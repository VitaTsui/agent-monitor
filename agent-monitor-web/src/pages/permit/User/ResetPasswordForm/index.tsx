import React from "react";
import { FormItemProps, Form } from "@hsu-react/ui";

interface ResetPasswordFormProps {
  open: boolean;
  username: string;
  onCancel: () => void;
  onOk: (username: string, password: string, callback: () => void) => void;
}

const ResetPasswordForm: React.FC<ResetPasswordFormProps> = ({
  open,
  username,
  onCancel,
  onOk,
}) => {
  const [form] = Form.useForm();

  const handleCancel = () => {
    form.resetFields();
    onCancel();
  };

  const handleOk = (data: Record<string, unknown>) => {
    delete data.confirmPassword;
    if (typeof data.password === "string") {
      onOk(username, data.password, () => {
        form.resetFields();
        handleCancel();
      });
    }
  };

  const formItems: FormItemProps[] = [
    {
      type: "PASSWORD",
      name: "password",
      label: "新密码",
      rules: [
        { required: true, message: "请输入密码" },
        { min: 6, message: "密码至少 6 位" },
      ],
    },
    {
      type: "PASSWORD",
      name: "confirmPassword",
      label: "确认密码",
      required: true,
      rules: [
        ({ getFieldValue }) => ({
          validator(_, value) {
            if (!value || getFieldValue("password") === value) {
              return Promise.resolve();
            }
            return Promise.reject("两次密码输入不一致");
          },
        }),
      ],
    },
  ];

  return (
    <Form.Modal
      open={open}
      title={`重置密码 - ${username}`}
      onCancel={handleCancel}
      onOk={handleOk}
      formItems={formItems}
      externalForm={form}
      onValuesChange={(changedValues) => {
        // 仅当确认密码字段有值时，当新密码字段变更时，触发确认密码字段的验证
        if (
          "password" in changedValues &&
          form &&
          form.getFieldValue("confirmPassword")
        ) {
          // 两次密码不一致时必然 reject（这正是此处联动校验的目的），
          // 错误已由 antd 渲染到表单项上，不吞掉会变成未捕获 rejection。
          form.validateFields(["confirmPassword"]).catch(() => void 0);
        }
      }}
    />
  );
};

export default ResetPasswordForm;
