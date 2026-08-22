pub(crate) struct ExactBoxNode<T> {
    value: T,
    next: Option<Box<Self>>,
}

pub(crate) struct ExactBoxList<T> {
    head: Option<Box<ExactBoxNode<T>>>,
}

impl<T> ExactBoxNode<T> {
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) const fn value(&self) -> &T {
        &self.value
    }
}

impl<T> Default for ExactBoxList<T> {
    fn default() -> Self {
        Self { head: None }
    }
}

impl<T> ExactBoxList<T> {
    pub(crate) fn boxed_node(value: T) -> Box<ExactBoxNode<T>> {
        Box::new(ExactBoxNode { value, next: None })
    }

    pub(crate) fn push_boxed(&mut self, mut node: Box<ExactBoxNode<T>>) {
        node.next = self.head.take();
        self.head = Some(node);
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        let mut next = self.head.as_deref();
        std::iter::from_fn(move || {
            let node = next?;
            next = node.next.as_deref();
            Some(&node.value)
        })
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        let mut next = self.head.as_deref_mut();
        std::iter::from_fn(move || {
            let node = next.take()?;
            next = node.next.as_deref_mut();
            Some(&mut node.value)
        })
    }

    pub(crate) fn retain(&mut self, mut retain: impl FnMut(&mut T) -> bool) {
        let mut link = &mut self.head;
        while let Some(mut node) = link.take() {
            if retain(&mut node.value) {
                *link = Some(node);
                link = &mut link.as_mut().expect("retained node was restored").next;
            } else {
                *link = node.next.take();
            }
        }
    }

    pub(crate) fn extract_if(&mut self, mut extract: impl FnMut(&mut T) -> bool) -> Self {
        let mut extracted = Self::default();
        let mut extracted_tail = &mut extracted.head;
        let mut link = &mut self.head;
        while let Some(mut node) = link.take() {
            if extract(&mut node.value) {
                *link = node.next.take();
                *extracted_tail = Some(node);
                extracted_tail = &mut extracted_tail
                    .as_mut()
                    .expect("extracted node was installed")
                    .next;
            } else {
                *link = Some(node);
                link = &mut link.as_mut().expect("retained node was restored").next;
            }
        }
        extracted
    }

    pub(crate) fn clear(&mut self) {
        let mut next = self.head.take();
        while let Some(mut node) = next {
            next = node.next.take();
        }
    }
}

impl<T> Drop for ExactBoxList<T> {
    fn drop(&mut self) {
        self.clear();
    }
}
