`
import {
  <NAME>Data,
  <NAME>SearchData,
  delete<NAME>,
  get<NAME>List,
} from "@/services/apis/<API_PATH>";

import ListPanelStore from "@/stores/basisStoreClass/ListPanelStore";
import { makeObservable } from "mobx";

class <NAME>Store extends ListPanelStore<<NAME>SearchData, <NAME>Data> {
  protected accessor _modeType = {
    <PARENT_ID_PROP>: "EQ",
  };

  constructor() {
    super();
    makeObservable(this);
  }

  /**
   * 获取列表
   */
  public getDataSource = () => {
    get<NAME>List({ query: this._query.value })
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
   * 删除
   * @param id 记录ID
   */
  public delData = (id: number | string) => {
    delete<NAME>(id).then((res) => {
      if (res.code === 0) {
        this.getDataSource();
      }

      this._message(res);
    });
  };
}

export default new <NAME>Store();
`;

