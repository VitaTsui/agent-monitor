import {
  UserData,
  UserSearchData,
  deleteUser,
  getUserCryptoKey,
  getUserList,
  resetUserPwd,
} from "@/services/apis/permit/user";

import ListPanelStore from "@/stores/basisStoreClass/ListPanelStore";
import crypto from "@/utils/crypto";
import { makeObservable } from "mobx";

class UserStore extends ListPanelStore<UserSearchData, UserData> {
  protected accessor _modeType = {
    username: "LK",
  };

  constructor() {
    super();
    makeObservable(this);
  }

  /**
   * 获取列表
   */
  public getDataSource = () => {
    getUserList({ query: this._query.value })
      .then((res) => {
        if (res.code === 0) {
          const { list, page } = res.data;
          const { total } = page;

          this._dataSource = list;
          this._total = total;
        } else {
          this._message(res);
        }

        this._isLoading = false;
      })
      .catch(() => {
        this._isLoading = false;
      });
  };

  /**
   * 删除（按用户名）
   */
  public delData = (username: number | string) => {
    deleteUser(username).then((res) => {
      if (res.code === 0) {
        this.getDataSource();
      }

      this._message(res);
    });
  };

  /**
   * 重置密码（按用户名；口令走 RSA+AES 加密，不明文过网络）
   */
  public resetUserPwd = async (
    username: string,
    password: string,
    fn?: () => void
  ) => {
    const keyRes = await getUserCryptoKey();
    if (keyRes.code !== 0) {
      this._message(keyRes);
      return;
    }
    const sessionKey = await crypto.decrypt(keyRes.data);
    const encrypted = await crypto.encodeRSA(
      await crypto.encrypt(password, sessionKey)
    );

    resetUserPwd({ username, password: encrypted, cryptoKey: keyRes.data }).then(
      (res) => {
        this._message(res);

        if (res.code === 0) {
          fn?.();
        }
      }
    );
  };
}

export default new UserStore();
