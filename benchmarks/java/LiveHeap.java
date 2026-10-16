public class LiveHeap {
    static final class Tree { Tree left, right; }
    static Tree make(int depth) {
        Tree t = new Tree();
        if (depth > 0) { t.left = make(depth - 1); t.right = make(depth - 1); }
        return t;
    }
    static int check(Tree t) { return t.left == null ? 1 : 1 + check(t.left) + check(t.right); }
    public static void main(String[] a) {
        Tree lng = make(22);
        int sum = 0;
        for (int i = 0; i < 20000; i++) sum += check(make(10));
        System.out.println("short lived: " + sum);
        System.out.println("long lived: " + check(lng));
    }
}
