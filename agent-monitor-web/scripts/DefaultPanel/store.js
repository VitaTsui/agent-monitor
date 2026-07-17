`
import { makeAutoObservable } from "mobx";

class <NAME>Store {
  constructor() {
    makeAutoObservable(this);
  }
}

export default new <NAME>Store();
`;
