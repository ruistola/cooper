public class BinaryTrees {
    static final class Tree { Tree left, right; }
    static Tree make(int depth) {
        Tree t = new Tree();
        if (depth > 0) { t.left = make(depth - 1); t.right = make(depth - 1); }
        return t;
    }
    static int check(Tree t) { return t.left == null ? 1 : 1 + check(t.left) + check(t.right); }
    public static void main(String[] a) {
        int maxDepth = 18;
        Tree lng = make(maxDepth);
        for (int d = 4; d <= maxDepth; d += 4) {
            int iters = 1 << (maxDepth - d + 4);
            int sum = 0;
            for (int i = 0; i < iters; i++) sum += check(make(d));
            System.out.println(iters + " trees of depth " + d + ": " + sum);
        }
        System.out.println("long lived: " + check(lng));
    }
}
