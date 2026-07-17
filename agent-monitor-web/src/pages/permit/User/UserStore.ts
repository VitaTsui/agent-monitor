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

  // 列表请求序号，用于丢弃乱序返回的过期响应（同基类 _rowRefreshSeq 的思路）
  private _listSeq = 0;

  /**
   * 获取列表
   */
  public getDataSource = () => {
    // 连续两次搜索时，先发的那次若返回更慢会覆盖后发的结果，
    // 让表格停在旧关键词的数据上。只认最后一次请求的响应。
    const seq = ++this._listSeq;

    getUserList({ query: this._query.value })
      .then((res) => {
        if (this._listSeq !== seq) {
          return;
        }
        if (res.code === 0) {
          // 后端异常时 data 可能不完整，解构前防御，避免整页白屏
          const { list, page } = res.data ?? {};

          this._dataSource = list ?? [];
          this._total = page?.total ?? 0;
        } else {
          this._message(res);
        }

        this._isLoading = false;
      })
      .catch(() => {
        if (this._listSeq !== seq) {
          return;
        }
        this._isLoading = false;
      });
  };

  /**
   * 删除（按用户名）
   */
  public delData = (username: number | string) => {
    deleteUser(username)
      .then((res) => {
        if (res.code === 0) {
          this.getDataSource();
        }

        this._message(res);
      })
      .catch(() => void 0);
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

    await resetUserPwd({
      username,
      password: encrypted,
      cryptoKey: keyRes.data,
    }).then((res) => {
      this._message(res);

      if (res.code === 0) {
        fn?.();
      }
    });
  };
}

export default new UserStore();
