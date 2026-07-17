import { PwdChangeData, editPwdChange } from "@/services/apis/pwdchange";

import FormModalStore from "@/stores/basisStoreClass/FormModalStore";
import { ResType } from "@/services/Axios";
import { makeObservable } from "mobx";
import { notification } from "antd";

class PwdChangeFormStore extends FormModalStore<PwdChangeData> {
  constructor() {
    super();
    makeObservable(this);
  }

  /**
   * 编辑
   * @param id
   * @param data
   * @param fn
   */
  public editFormData = (
    _id: number | string,
    data: PwdChangeData,
    fn?: (res: ResType) => void
  ) => {
    editPwdChange(data).then((res) => {
      if (res.code === 0) {
        fn?.(res);

        notification.success({
          message: "修改成功，请重新登录",
        });
      } else {
        this._message(res);
      }
    });
  };
}

export default new PwdChangeFormStore();
