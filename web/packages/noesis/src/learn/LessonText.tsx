// How lesson prose renders: every teacher, learner, question and problem text goes through here, so
// a richer renderer replaces it in one place.

export function LessonText({ text }: { readonly text: string }) {
  return <>{text}</>;
}
