-- MySQL Test Database Initialization Script
-- Creates sample tables with test data for datasource testing

-- Customers table
CREATE TABLE customers (
    customer_id INT PRIMARY KEY AUTO_INCREMENT,
    first_name VARCHAR(50) NOT NULL,
    last_name VARCHAR(50) NOT NULL,
    email VARCHAR(100) UNIQUE NOT NULL,
    country VARCHAR(50),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Products table
CREATE TABLE products (
    product_id INT PRIMARY KEY AUTO_INCREMENT,
    product_name VARCHAR(100) NOT NULL,
    category VARCHAR(50),
    price DECIMAL(10, 2) NOT NULL,
    stock_quantity INT DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Orders table
CREATE TABLE orders (
    order_id INT PRIMARY KEY AUTO_INCREMENT,
    customer_id INT NOT NULL,
    order_date DATE NOT NULL,
    total_amount DECIMAL(10, 2),
    status VARCHAR(20) DEFAULT 'pending',
    FOREIGN KEY (customer_id) REFERENCES customers(customer_id)
);

-- Order items table
CREATE TABLE order_items (
    order_item_id INT PRIMARY KEY AUTO_INCREMENT,
    order_id INT NOT NULL,
    product_id INT NOT NULL,
    quantity INT NOT NULL,
    unit_price DECIMAL(10, 2) NOT NULL,
    FOREIGN KEY (order_id) REFERENCES orders(order_id),
    FOREIGN KEY (product_id) REFERENCES products(product_id)
);

-- Insert sample customers
INSERT INTO customers (first_name, last_name, email, country) VALUES
('John', 'Doe', 'john.doe@example.com', 'USA'),
('Jane', 'Smith', 'jane.smith@example.com', 'Canada'),
('Bob', 'Johnson', 'bob.johnson@example.com', 'UK'),
('Alice', 'Williams', 'alice.williams@example.com', 'Australia'),
('Charlie', 'Brown', 'charlie.brown@example.com', 'USA'),
('Diana', 'Davis', 'diana.davis@example.com', 'Germany'),
('Eve', 'Miller', 'eve.miller@example.com', 'France'),
('Frank', 'Wilson', 'frank.wilson@example.com', 'Japan');

-- Insert sample products
INSERT INTO products (product_name, category, price, stock_quantity) VALUES
('Laptop Pro 15"', 'Electronics', 1299.99, 50),
('Wireless Mouse', 'Electronics', 29.99, 200),
('USB-C Cable', 'Accessories', 12.99, 500),
('Ergonomic Keyboard', 'Electronics', 89.99, 100),
('Monitor 27"', 'Electronics', 349.99, 75),
('Desk Lamp', 'Office', 45.99, 150),
('Notebook Set', 'Office', 15.99, 300),
('Water Bottle', 'Lifestyle', 19.99, 250),
('Backpack', 'Lifestyle', 59.99, 120),
('Coffee Mug', 'Lifestyle', 12.99, 400);

-- Insert sample orders
INSERT INTO orders (customer_id, order_date, total_amount, status) VALUES
(1, '2024-01-15', 1342.97, 'completed'),
(2, '2024-01-16', 89.99, 'completed'),
(3, '2024-01-17', 425.96, 'shipped'),
(1, '2024-01-18', 45.99, 'completed'),
(4, '2024-01-19', 1699.97, 'processing'),
(5, '2024-01-20', 32.98, 'completed'),
(2, '2024-01-21', 379.98, 'shipped'),
(6, '2024-01-22', 102.97, 'completed'),
(7, '2024-01-23', 59.99, 'processing'),
(8, '2024-01-24', 1349.98, 'completed');

-- Insert sample order items
INSERT INTO order_items (order_id, product_id, quantity, unit_price) VALUES
-- Order 1 (John Doe)
(1, 1, 1, 1299.99),
(1, 3, 2, 12.99),
(1, 2, 1, 29.99),
-- Order 2 (Jane Smith)
(2, 4, 1, 89.99),
-- Order 3 (Bob Johnson)
(3, 5, 1, 349.99),
(3, 6, 1, 45.99),
(3, 2, 1, 29.99),
-- Order 4 (John Doe)
(4, 6, 1, 45.99),
-- Order 5 (Alice Williams)
(5, 1, 1, 1299.99),
(5, 5, 1, 349.99),
(5, 4, 1, 89.99),
-- Order 6 (Charlie Brown)
(6, 8, 1, 19.99),
(6, 3, 1, 12.99),
-- Order 7 (Jane Smith)
(7, 5, 1, 349.99),
(7, 2, 1, 29.99),
-- Order 8 (Diana Davis)
(8, 7, 3, 15.99),
(8, 8, 2, 19.99),
(8, 10, 2, 12.99),
-- Order 9 (Eve Miller)
(9, 9, 1, 59.99),
-- Order 10 (Frank Wilson)
(10, 1, 1, 1299.99),
(10, 4, 1, 89.99);
