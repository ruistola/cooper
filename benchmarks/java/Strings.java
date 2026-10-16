import java.util.*;
public class Strings {
    public static void main(String[] a) {
        long total = 0;
        for (int round = 0; round < 60; round++) {
            List<String> parts = new ArrayList<>();
            for (int i = 0; i < 20000; i++) parts.add("item " + i + " of round " + round);
            String text = String.join(",", parts);
            total += text.length();
            total += text.split(",").length;
        }
        System.out.println(total);
    }
}
