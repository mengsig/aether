// @aether-ui/home: the overview the shell opens on.

// Loaded on demand by the shell, so the home's tiles and charts stay out of the entry chunk.
export const loadHomeScreen = () => import("./overview/HomeScreen.tsx");
