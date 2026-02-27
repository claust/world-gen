pub trait Layer<I, O> {
    fn generate(&self, input: I) -> O;
}
