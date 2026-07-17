`
import {
  <NAME>Data,
  create<NAME>,
  edit<NAME>,
  get<NAME>,
} from "@/services/apis/<API_PATH>";

import FormModalStore from "@/stores/basisStoreClass/FormModalStore";
import { ResType } from "@/services/Axios";
import { makeObservable } from "mobx";

class <NAME>FormStore extends FormModalStore<<NAME>Data> {
  constructor() {
    super();
    makeObservable(this);
  }

  /**
   * 获取详情
   * @param id 记录ID
   */
  protected _getFormData = (id: number | string) => {
    get<NAME>(id).then((res) => {
      if (res.code === 0) {
        this._formData = res.data;
      } else {
        this._message(res);
      }
    });
  };

  /**
   * 新增
   * @param data 数据
   * @param fn 成功回调
   */
  public addFormData = (
    data: <NAME>Data,
    fn?: (res: ResType) => void
  ) => {
    create<NAME>(data).then((res) => {
      if (res.code === 0) {
        fn?.(res);
      }

      this._message(res);
    });
  };

  /**
   * 编辑
   * @param id 记录ID
   * @param data 数据
   * @param fn 成功回调
   */
  public editFormData = (
    id: number | string,
    data: <NAME>Data,
    fn?: (res: ResType) => void
  ) => {
    data.id = id;

    edit<NAME>(data).then((res) => {
      fn?.(res);

      this._message(res);
    });
  };
}

export default new <NAME>FormStore();
`;
