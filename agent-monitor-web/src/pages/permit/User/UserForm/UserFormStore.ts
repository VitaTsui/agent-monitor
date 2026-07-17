import {
  UserData,
  createUser,
  editUser,
  getUserCryptoKey,
} from "@/services/apis/permit/user";

import FormModalStore from "@/stores/basisStoreClass/FormModalStore";
import crypto from "@/utils/crypto";
import { makeObservable } from "mobx";

class UserFormStore extends FormModalStore<UserData> {
  constructor() {
    super();
    makeObservable(this);
  }

  /**
   * 详情：hub 无单独详情接口，直接使用列表行数据回填
   */
  protected _getFormData = (_id: number | string, data?: UserData) => {
    if (data) {
      this._formData = data;
    }
  };

  /**
   * 新增（口令走 RSA+AES 加密，与登录同方案，不明文过网络）
   */
  public addFormData = async (
    data: UserData & { password?: string },
    fn?: () => void
  ) => {
    const keyRes = await getUserCryptoKey();
    if (keyRes.code !== 0) {
      this._message(keyRes);
      return;
    }
    const sessionKey = await crypto.decrypt(keyRes.data);
    const password = await crypto.encodeRSA(
      await crypto.encrypt(String(data.password ?? ""), sessionKey)
    );

    createUser({
      username: data.username,
      nickname: data.nickname,
      password,
      cryptoKey: keyRes.data,
    }).then((res) => {
      if (res.code === 0) {
        fn?.();
      }

      this._message(res);
    });
  };

  /**
   * 修改（昵称）
   */
  public editFormData = (
    username: number | string,
    data: UserData,
    fn?: () => void
  ) => {
    editUser({ username: String(username), nickname: data.nickname }).then(
      (res) => {
        if (res.code === 0) {
          fn?.();
        }

        this._message(res);
      }
    );
  };
}

export default new UserFormStore();
