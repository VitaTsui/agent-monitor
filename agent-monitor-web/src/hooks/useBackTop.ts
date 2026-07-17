export default function useBackTop(id: string = "#App") {
  const App = document.querySelector(id) as HTMLDivElement;

  const scrollTop = App?.scrollTop || 0;

  const backTop = () => {
    if (App) {
      App.scrollTop = 0;
    }
  };

  return { scrollTop, backTop };
}
