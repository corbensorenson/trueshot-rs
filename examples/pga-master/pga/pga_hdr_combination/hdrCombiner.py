class hdr_combiner:
    def __init__(self, implementation = "hdrFusion"):
        self.implementation = implementation

    def combine(self, images):
        self.implementation.combine(images)

    def combine_multiple(self, imageArrays):
        answer = []
        for images in imageArrays:
            answer.append(self.implementation.combine(images))
        return answer
