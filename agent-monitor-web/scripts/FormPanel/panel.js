`
import React, { useEffect } from "react";

import { FormItemProps, Form } from "@hsu-react/ui";
import <NAME>FormStore from "./<NAME>FormStore";
import { observer } from "mobx-react-lite";
import styles from "./index.module.scss";

interface <NAME>FormProps {
  open?: boolean;
  title?: string;
  id?: string | number;
  onCancel?: () => void;
  onOk?: () => void;
}

const <NAME>Form: React.FC<<NAME>FormProps> = observer((props) => {
  const { open, title, id, onCancel, onOk } = props;
  const { resetFormData, addFormData, editFormData, formData, getFormData } =
    <NAME>FormStore;

  useEffect(() => {
    if (id && open) {
      getFormData(id);
    }
  }, [getFormData, id, open]);

  const formItems: FormItemProps[] = [];

  const onClose = () => {
    resetFormData();
    onCancel?.();
  };

  const handleOk = (data: Record<string, unknown>) => {
    if (id) {
      editFormData(id, data, () => {
        onClose();
        onOk?.();
      });
    } else {
      addFormData(data, () => {
        onClose();
        onOk?.();
      });
    }
  };

  return (
    <Form.Modal
      className={styles.<NAME>Form}
      title={title}
      open={open}
      onCancel={onClose}
      onOk={handleOk}
      formItems={formItems}
      value={formData}
    />
  );
});

export default <NAME>Form;
`;
