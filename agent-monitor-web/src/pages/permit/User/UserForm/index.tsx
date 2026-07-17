import React, { useEffect } from "react";

import { Form, FormItemProps } from "@hsu-react/ui";
import { UserData } from "@/services/apis/permit/user";
import UserFormStore from "./UserFormStore";
import { observer } from "mobx-react-lite";
import styles from "./index.module.scss";

interface UserFormProps {
  open?: boolean;
  title?: string;
  /** 编辑时传入的行数据；缺省为新增 */
  data?: UserData;
  onCancel?: () => void;
  onOk?: () => void;
}

const UserForm: React.FC<UserFormProps> = observer((props) => {
  const { open, title, data, onCancel, onOk } = props;
  const { resetFormData, addFormData, editFormData, formData, getFormData } =
    UserFormStore;
  const [form] = Form.useForm();

  const isEdit = !!data?.username;

  useEffect(() => {
    if (open && data?.username) {
      getFormData(data.username, data);
    }
  }, [getFormData, open, data]);

  const formItems: FormItemProps[] = [
    {
      type: "INPUT",
      name: "username",
      label: "用户名",
      required: !isEdit,
      componentProps: {
        disabled: isEdit,
        placeholder: "登录账号，创建后不可修改",
      },
    },
    {
      type: "PASSWORD",
      name: "password",
      label: "初始密码",
      required: !isEdit,
      visible: !isEdit,
      rules: [{ min: 6, message: "密码至少 6 位" }],
    },
    { type: "INPUT", name: "nickname", label: "昵称" },
  ];

  const onClose = () => {
    form.resetFields();
    resetFormData();
    onCancel?.();
  };

  const handleOk = (values: Record<string, unknown>) => {
    if (isEdit) {
      editFormData(data?.username ?? "", values, () => {
        onClose();
        onOk?.();
      });
    } else {
      addFormData(values, () => {
        onClose();
        onOk?.();
      });
    }
  };

  return (
    <Form.Modal
      className={styles.UserForm}
      title={title}
      open={open}
      onCancel={onClose}
      onOk={handleOk}
      formItems={formItems}
      value={formData}
      externalForm={form}
    />
  );
});

export default UserForm;
